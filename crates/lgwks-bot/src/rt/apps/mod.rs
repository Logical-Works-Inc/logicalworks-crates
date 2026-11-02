//! Model-written plans over the tenant HTTP edge (#786).
//!
//! A model writes a plan as `script!` text or as JSON of the same tree and
//! posts it to `POST /t/{tenant}/apps`. The edge admits it against the
//! tenant's [`Surface`](crate::proposal::Surface) — every operation the plan
//! names must be registered and every capability it needs already held — and
//! answers `{"app": "<id>"}`. Anything else is the full list of refusals:
//! unknown operation, missing capability, bound exceeded, each naming its
//! location (`line:col` for text, a JSON pointer for JSON), gathered across
//! the whole plan and sorted rather than stopped at the first.
//!
//! Bounds come before the parse in two layers. The byte count and the line
//! count are charged before anything is parsed, so an oversized body is
//! refused before it is buffered past the limit; the structural ceilings —
//! nesting depth, step count and fan-out — are measured by an iterative walk
//! over the parsed shapes before any operation is authorized. The walk is
//! iterative so a hostile nesting depth cannot exhaust the stack, while the
//! recursive reader it protects stays within the line-count ceiling.
//!
//! An admitted plan runs the host's real handlers through the same
//! [`graph`](crate::script::graph) scheduler, step scopes and run store the
//! compiled `script!` path uses. The parsed tree is lowered into
//! [`GraphNode`](crate::script::GraphNode)s — what the macro emits — with the
//! graph's own declared edges; steps that declare none run as roots. Each
//! node calls its registered handler inside the node's own step scope, and
//! their lines read back in declaration order. There is no second evaluator:
//! a plan that needs a word this lowering does not execute is refused at
//! admission, and a plan can never name a capability its tenant was not
//! granted, because the names admitted and the names runnable are one
//! registry.
//!
//! App ids are scoped by tenant: the store keys every plan by
//! `(tenant, id)`, so two tenants holding the same id stay isolated.

mod admit_json;
mod admit_text;
mod limits;
mod refusal;
mod routes;
mod run;
mod store;

pub use limits::PlanLimits;
pub use refusal::{PlanRefusal, RefusalKind};
pub use routes::{HeldGrants, apps_routes};
pub use run::{RunError, RunKind, run_app};
pub use store::{
    Admitted, AdmittedOp, AppStore, OpHandler, OpRegistry, Outcome, Refused, RegistryError,
};
