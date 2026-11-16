//! The application flow: ten durable steps, the code a user would write.
//!
//! This file is what `flow_code_lines` counts, so it holds the flow and nothing
//! else: the peers, the failure plan and the measuring live in their own files.
//! Every step is behind `remember`, so a restarted run replays what it finished.
//!
//! | step | what it does |
//! |---|---|
//! | 1, 2, 4, 5, 7, 9, 10 | local work, one durable record each |
//! | 3 | one outside call, retried under the crate's policy |
//! | 6 | one model call, retried under the crate's policy |
//! | 8 | one approval: the run parks until a decision is recorded |

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::script::{FlowError, Scope, approval, remember, retry};
use lgwks_bot::task::{Task, task};

use crate::world::{ATTEMPTS, Call, STEP_WEIGHTS, World};

/// The future one run of the flow is.
pub(crate) type FlowFuture = Pin<Box<dyn Future<Output = Result<u64, FlowError>>>>;

/// The body a task carries, behind one nameable type.
type FlowBody = Box<dyn Fn(Scope, u32) -> FlowFuture + Send + Sync>;

/// The task one host runs.
pub(crate) type FlowTask = Task<FlowBody>;

/// The wait before a retry doubles from this.
const BACKOFF: Duration = Duration::from_millis(1);

/// How long a parked approval waits for its decision.
const APPROVAL_LIMIT: Duration = Duration::from_secs(600);

/// The flow as a task named `durable-flow`, over `world`.
///
/// # Errors
///
/// [`FlowError::InvalidName`] if the name is refused.
pub(crate) fn flow_task(world: Arc<World>) -> Result<FlowTask, FlowError> {
    let body: FlowBody =
        Box::new(move |scope, ordinal| Box::pin(ten_steps(Arc::clone(&world), scope, ordinal)));
    task("durable-flow", body)
}

/// The ten steps of run `ordinal`; the value is the running total.
async fn ten_steps(world: Arc<World>, scope: Scope, ordinal: u32) -> Result<u64, FlowError> {
    let world = world.as_ref();
    let [w1, w2, w4, w5, w7, w9, w10] = STEP_WEIGHTS;
    let mut total = u64::from(ordinal);
    total = local(world, &scope, ordinal, "s1-intake", (total, w1)).await?;
    total = local(world, &scope, ordinal, "s2-validate", (total, w2)).await?;
    total = called(world, &scope, ordinal, "s3-outside", (total, Call::Outside)).await?;
    total = local(world, &scope, ordinal, "s4-merge", (total, w4)).await?;
    total = local(world, &scope, ordinal, "s5-prompt", (total, w5)).await?;
    total = called(world, &scope, ordinal, "s6-model", (total, Call::Model)).await?;
    total = local(world, &scope, ordinal, "s7-check", (total, w7)).await?;
    let payload = u64::from(ordinal).to_le_bytes();
    let approved = approval(&scope, "s8-approval", "release", &payload, APPROVAL_LIMIT).await?;
    let held = u64::try_from(approved.payload().len())
        .map_err(|error| FlowError::failed(format!("the payload length: {error}")))?;
    total = total.wrapping_add(held);
    total = local(world, &scope, ordinal, "s9-apply", (total, w9)).await?;
    local(world, &scope, ordinal, "s10-finish", (total, w10)).await
}

/// A step that only computes: it adds its weight, durably.
async fn local(
    world: &World,
    scope: &Scope,
    ordinal: u32,
    step: &'static str,
    (total, weight): (u64, u64),
) -> Result<u64, FlowError> {
    remember(scope, step, || async move {
        world.ran(scope.tenant().as_str(), ordinal, step)?;
        Ok(total.wrapping_add(weight))
    })
    .await
}

/// A step that makes one call to a peer, retried while the failure is safe to
/// retry, and records the value it brought back.
async fn called(
    world: &World,
    scope: &Scope,
    ordinal: u32,
    step: &'static str,
    (total, call): (u64, Call),
) -> Result<u64, FlowError> {
    remember(scope, step, || async move {
        world.ran(scope.tenant().as_str(), ordinal, step)?;
        let value = retry(
            scope,
            call.label(),
            ATTEMPTS,
            BACKOFF,
            |_attempt_scope, attempt| async move { world.call(call, ordinal, attempt).await },
        )
        .await?;
        Ok(total.wrapping_add(value))
    })
    .await
}
