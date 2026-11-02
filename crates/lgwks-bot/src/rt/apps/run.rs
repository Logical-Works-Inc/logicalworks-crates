//! Running an admitted plan through the graph scheduler.

// ── running ──────────────────────────────────────────────────────────────────

use std::sync::Arc;

use crate::script::{GraphNode, NodeOutcome, Scope, graph};

use super::store::AppStore;

/// Why a stored plan did not run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum RunKind {
    /// No plan is admitted under this id for this tenant.
    UnknownApp,
    /// One operation failed or never ran.
    NodeFailed,
    /// The lowered graph itself is not runnable.
    GraphFailed,
    /// The plan store itself is unavailable.
    StoreUnavailable,
}

impl RunKind {
    /// The name this failure carries in a log.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        match *self {
            Self::UnknownApp => "unknown-app",
            Self::NodeFailed => "node-failed",
            Self::GraphFailed => "graph-failed",
            Self::StoreUnavailable => "store-unavailable",
        }
    }
}

/// Why a stored plan did not run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunError {
    /// Which arm fired.
    kind: RunKind,
    /// What it names.
    detail: String,
}

impl RunError {
    /// Which arm fired.
    #[must_use]
    pub const fn kind(&self) -> RunKind {
        self.kind
    }

    /// What it names.
    #[must_use]
    pub fn detail(&self) -> &str {
        self.detail.as_str()
    }

    /// No plan is admitted under `id` for `tenant`.
    fn unknown(tenant: &str, id: &str) -> Self {
        Self {
            kind: RunKind::UnknownApp,
            detail: format!("no admitted app {id:?} for tenant {tenant:?}"),
        }
    }

    /// The operation `node` failed with `reason`.
    fn node(node: &str, reason: String) -> Self {
        Self {
            kind: RunKind::NodeFailed,
            detail: format!("operation {node:?} failed: {reason}"),
        }
    }

    /// The lowered graph itself is not runnable: `reason`.
    fn graph(reason: String) -> Self {
        Self {
            kind: RunKind::GraphFailed,
            detail: format!("the admitted graph is not runnable: {reason}"),
        }
    }

    /// The plan store itself is unavailable.
    pub(crate) fn store() -> Self {
        Self {
            kind: RunKind::StoreUnavailable,
            detail: String::from("the plan store is unavailable"),
        }
    }
}

impl std::fmt::Display for RunError {
    /// The detail.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.detail.as_str())
    }
}

impl std::error::Error for RunError {}

/// Run the plan `id` for the scope's tenant, printing one line per operation.
///
/// The operations run as [`GraphNode`](crate::script::GraphNode)s through
/// [`graph`](crate::script::graph) — the scheduler, step scopes and trail
/// the compiled `script!` path uses — with the graph's own declared edges.
/// Each node calls its registered handler inside the node's own step scope,
/// and the printed lines read back in declaration order. The run is
/// deterministic for one plan, and dropping the stored plan frees every
/// name the run borrowed from it.
///
/// # Errors
///
/// [`RunKind::UnknownApp`] when no plan is admitted under `id` for this
/// tenant; [`RunKind::NodeFailed`] when an operation fails; and
/// [`RunKind::GraphFailed`] when the lowered graph is not runnable.
pub async fn run_app(scope: &Scope, store: &AppStore, id: &str) -> Result<Vec<Vec<u8>>, RunError> {
    let tenant = scope.tenant().as_str().to_owned();
    let planned = match store.plan(tenant.as_str(), id) {
        Ok(planned) => planned,
        Err(()) => {
            lgwks_std::trace::debug!("apps: the plan store lock is poisoned");
            return Err(RunError::store());
        }
    };
    let Some(planned) = planned else {
        lgwks_std::trace::debug!(app = id, "apps: no admitted app under this id");
        return Err(RunError::unknown(tenant.as_str(), id));
    };
    if planned.ops.is_empty() {
        lgwks_std::trace::debug!(app = id, "apps: the stored plan names nothing");
        return Err(RunError::graph(String::from(
            "the stored plan names no operation",
        )));
    }
    let mut nodes = Vec::with_capacity(planned.ops.len());
    for op in &planned.ops {
        let handler = op.handler.clone();
        let input = op.input.clone();
        let needs: Vec<&str> = op.needs.iter().map(AsRef::as_ref).collect();
        nodes.push(GraphNode::new(
            Arc::clone(&op.exec),
            needs.as_slice(),
            move |scope: Scope| {
                let handler = handler.clone();
                let input = input.clone();
                Box::pin(async move { handler.call(scope, input).await })
            },
        ));
    }
    let report = match graph(scope, "plan", planned.ops.len(), nodes).await {
        Ok(report) => report,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "apps: the admitted graph is refused");
            return Err(RunError::graph(error.to_string()));
        }
    };
    let mut lines = Vec::with_capacity(planned.ops.len());
    for op in &planned.ops {
        let Some(outcome) = report.outcome(op.exec.as_ref()) else {
            lgwks_std::trace::debug!(node = ?op.exec, "apps: the report names no such operation");
            return Err(RunError::graph(format!(
                "the report names no operation {:?}",
                op.exec
            )));
        };
        match *outcome {
            NodeOutcome::Ran(Ok(ref line)) => lines.push(line.clone()),
            NodeOutcome::Ran(Err(ref error)) => {
                lgwks_std::trace::debug!(node = ?op.exec, error = ?error, "apps: the operation failed");
                return Err(RunError::node(op.exec.as_ref(), error.to_string()));
            }
            NodeOutcome::Blocked { ref by } => {
                lgwks_std::trace::debug!(node = ?op.exec, by = ?by, "apps: the operation never ran");
                return Err(RunError::node(
                    op.exec.as_ref(),
                    format!("blocked by {}", by.join(", ")),
                ));
            }
        }
    }
    Ok(lines)
}
