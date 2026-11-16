//! `lgwks-bench-restate-worker`: the Restate row's application, served over HTTP
//! for `restate-server` to call.
//!
//! The driver (`lgwks-bench-durable --rows restate`) starts this process, passes
//! the loopback port, the failure plan it drew from the seed and the log files,
//! and registers the endpoint with the server. This process prints one notice line
//! on stdout when a run parks at its approval and one when a run ends lost. It
//! ends when its stdin closes, so it never outlives the driver, or when it is
//! killed.
//!
//! Exit status: 0 on a clean end, 1 on a failure, 2 on a bad command line.

use std::io::Write;
use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use lgwks_bot::clock::Clock;
use restate_sdk::prelude::*;

mod flow;
#[path = "../durable/rival_wire.rs"]
mod rival_wire;
#[cfg(test)]
mod sim_world;
mod world;

use flow::DurableFlow;
use world::{World, WorldError};

/// Why the worker ended.
#[derive(Debug)]
enum WorkerError {
    /// The command line is wrong.
    Usage {
        /// How.
        detail: String,
    },
    /// The world could not be set up.
    World {
        /// The refusal.
        source: WorldError,
    },
    /// A socket, the runtime or stdin failed.
    Io {
        /// What was being done.
        context: &'static str,
        /// What the OS said.
        source: std::io::Error,
    },
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Usage { ref detail } => write!(formatter, "usage: {detail}"),
            Self::World { ref source } => write!(formatter, "{source}"),
            Self::Io { context, ref source } => write!(formatter, "{context}: {source}"),
        }
    }
}

/// What the command line asked for.
struct Settings {
    /// The loopback port to serve on.
    port: u16,
    /// The failure plan file.
    plan: PathBuf,
    /// The step log, when step bodies are to be recorded.
    steps: Option<PathBuf>,
    /// The attempt log.
    attempts: PathBuf,
}

/// The value after a flag.
fn value(flag: &str, next: Option<String>) -> Result<String, WorkerError> {
    next.ok_or_else(|| WorkerError::Usage {
        detail: format!("{flag} needs a value"),
    })
}

/// Parse the arguments after the program name.
fn parse(items: impl IntoIterator<Item = String>) -> Result<Settings, WorkerError> {
    let (mut port, mut plan, mut steps, mut attempts) = (None, None, None, None);
    let mut cursor = items.into_iter();
    while let Some(word) = cursor.next() {
        match word.as_str() {
            "--port" => {
                let text = value("--port", cursor.next())?;
                port = Some(text.parse::<u16>().map_err(|source| WorkerError::Usage {
                    detail: format!("--port {text:?}: {source}"),
                })?);
            }
            "--plan" => plan = Some(PathBuf::from(value("--plan", cursor.next())?)),
            "--steps" => steps = Some(PathBuf::from(value("--steps", cursor.next())?)),
            "--attempts" => attempts = Some(PathBuf::from(value("--attempts", cursor.next())?)),
            other => {
                lgwks_std::trace::debug!(argument = other, "no flag has this name");
                return Err(WorkerError::Usage {
                    detail: format!("no flag named {other:?}"),
                });
            }
        }
    }
    let missing = |flag: &str| WorkerError::Usage {
        detail: format!("{flag} is required"),
    };
    Ok(Settings {
        port: port.ok_or_else(|| missing("--port"))?,
        plan: plan.ok_or_else(|| missing("--plan"))?,
        steps,
        attempts: attempts.ok_or_else(|| missing("--attempts"))?,
    })
}

/// Serve the flow until stdin closes, waiting the upstream latencies on `clock`.
fn serve(settings: &Settings, clock: Clock) -> Result<(), WorkerError> {
    let world = World::load(
        &settings.plan,
        settings.steps.as_deref(),
        &settings.attempts,
        clock,
    )
    .map_err(|source| WorkerError::World { source })?;
    let io = |context| move |source| WorkerError::Io { context, source };
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, settings.port));
    let listener = TcpListener::bind(address).map_err(io("binding the endpoint"))?;
    listener
        .set_nonblocking(true)
        .map_err(io("making the listener non-blocking"))?;
    let (gone, closed) = tokio::sync::oneshot::channel::<()>();
    std::thread::Builder::new()
        .name("lgwks-bench-restate-stdin".to_owned())
        .spawn(move || {
            if let Err(error) = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink()) {
                lgwks_std::trace::debug!(%error, "stdin failed; the worker stops");
            }
            if gone.send(()).is_err() {
                lgwks_std::trace::debug!("the worker had already stopped");
            }
        })
        .map_err(io("starting the stdin watcher"))?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(io("building the runtime"))?;
    let endpoint = Endpoint::builder()
        .bind(DurableFlow::new(Arc::new(world)))
        .build();
    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(listener).map_err(io("adopting the listener"))?;
        HttpServer::new(endpoint)
            .serve_with_cancel(listener, async move {
                if closed.await.is_err() {
                    lgwks_std::trace::debug!("the stdin watcher ended without a signal");
                }
            })
            .await;
        Ok::<(), WorkerError>(())
    })
}

fn main() -> ExitCode {
    let outcome = parse(std::env::args().skip(1)).and_then(|settings| serve(&settings, Clock::wall()));
    match outcome {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let code = if matches!(error, WorkerError::Usage { .. }) { 2 } else { 1 };
            let mut err = std::io::stderr().lock();
            if writeln!(err, "lgwks-bench-restate-worker: {error}").is_err() {
                return ExitCode::from(3);
            }
            ExitCode::from(code)
        }
    }
}
