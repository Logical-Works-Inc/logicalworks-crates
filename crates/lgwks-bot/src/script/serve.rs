//! The `serve` word's runtime: the tenant HTTP edge as a flow step.
//!
//! The language reads the words; this module runs them. [`serve_at`] serves
//! the routes a `serve` block declared, over [`ServeOptions`], and answers
//! [`ServeReport`]. The heavy edge lives in [`crate::rt::serve`]; this is the
//! thin flow step over it, so the word expands to one call here.

pub use crate::rt::serve::{ServeError, ServeOptions, ServeReport};

use crate::script::{FlowError, Scope, Site};

/// Serve the routes a `serve` block declared, until the scope stops or the
/// process is asked to stop.
///
/// Enters the step's own scope, resolves the port and ceiling the word
/// declared, serves every `route` under it with tenant isolation and
/// idempotency, drains within the declared deadline on stop, and reports
/// what drained and what was cancelled. Called by `script!`.
///
/// `SIGTERM` and `SIGINT` (Ctrl-C off Unix) are a stop, armed before the
/// listener binds. Before this, the default action ended the process at
/// once: no drain ran, and an ephemeral run store stayed in the temp dir for
/// ever (the residue census, ADR 0078). A supervisor, `lgwks run`'s watchdog,
/// systemd and `docker stop` all send `SIGTERM`.
pub async fn serve_at(
    scope: &Scope,
    step: &str,
    site: &'static Site,
    options: ServeOptions,
    routes: Vec<crate::rt::serve::TypedRoute>,
) -> Result<ServeReport, FlowError> {
    let child = scope.enter_at(step, site)?;
    let mut watchers = lgwks_deps::tokio::task::JoinSet::new();
    watch_stop_signals(child.token(), &mut watchers).map_err(|cause| {
        lgwks_std::trace::debug!(%cause, "serve: the stop signals could not be armed");
        FlowError::failed(format!(
            "the serve stop signals could not be armed: {cause}"
        ))
    })?;
    let port = options.port();
    let listener = std::net::TcpListener::bind(format!("127.0.0.1:{port}")).map_err(|cause| {
        FlowError::failed(format!("the serve listener could not bind: {cause}"))
    })?;
    let outcome = crate::rt::serve::serve_async(options, routes, listener, child.token().clone())
        .await
        .map_err(|cause| FlowError::failed(format!("the serve edge refused: {cause}")));
    // The watchers end with the serve: owned by the set, aborted here.
    watchers.abort_all();
    child.settle(&outcome);
    outcome.map_err(|error| error.located(&child))
}

/// Cancel `stop` when the process is asked to stop: one owned watcher per
/// signal, in `watchers`.
///
/// # Errors
///
/// A signal stream the OS refused; the serve does not start without it.
fn watch_stop_signals(
    stop: &crate::rt::sync::CancellationToken,
    watchers: &mut lgwks_deps::tokio::task::JoinSet<()>,
) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use crate::rt::signal::unix::{SignalKind, signal};
        for kind in [SignalKind::terminate(), SignalKind::interrupt()] {
            let mut stream = signal(kind)?;
            let token = stop.clone();
            watchers.spawn(async move {
                if stream.recv().await.is_some() {
                    lgwks_std::trace::debug!("serve: a stop signal arrived; draining");
                    token.cancel();
                }
            });
        }
    }
    #[cfg(windows)]
    {
        let token = stop.clone();
        watchers.spawn(async move {
            match crate::rt::signal::ctrl_c().await {
                Ok(()) => token.cancel(),
                Err(cause) => {
                    lgwks_std::trace::debug!(%cause, "serve: Ctrl-C could not be watched");
                }
            }
        });
    }
    #[cfg(not(any(unix, windows)))]
    {
        lgwks_std::trace::debug!(
            stopped = stop.is_cancelled(),
            watchers = watchers.len(),
            "serve: this target has no stop signals; the scope alone stops it"
        );
    }
    Ok(())
}
