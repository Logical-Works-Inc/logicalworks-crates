//! The application flow on Restate: ten durable steps, the code a Restate user
//! would write. The counterpart of `durable/flow.rs`, and the file the report
//! counts for the Restate row's code-size line.
//!
//! | step | what it does |
//! |---|---|
//! | 1, 2, 4, 5, 7, 9, 10 | local work, one `ctx.run` journal entry each |
//! | 3 | one outside call, retried under the run's retry policy |
//! | 6 | one model call, retried under the run's retry policy |
//! | 8 | one approval: a durable promise the run waits on |

use std::sync::Arc;
use std::time::Duration;

use restate_sdk::prelude::*;

use crate::rival_wire::parse_key;
use crate::world::{ATTEMPTS, Call, STEP_WEIGHTS, World};

/// The wait before a retry doubles from this.
const BACKOFF: Duration = Duration::from_millis(1);

/// The promise a decision resolves.
const APPROVED: &str = "approved";

/// The flow's service: the workflow `DurableFlow`, keyed `<tenant>-<run>`.
pub(crate) struct DurableFlow {
    /// What the flow reaches that is not Restate.
    world: Arc<World>,
}

impl DurableFlow {
    /// The flow over `world`.
    pub(crate) const fn new(world: Arc<World>) -> Self {
        Self { world }
    }

    /// A step that only computes: it adds its weight, durably.
    async fn local(
        &self,
        ctx: &WorkflowContext<'_>,
        (tenant, ordinal): (&str, u32),
        step: &'static str,
        (total, weight): (u64, u64),
    ) -> Result<u64, HandlerError> {
        let (world, tenant) = (Arc::clone(&self.world), tenant.to_owned());
        let sum = ctx
            .run(|| async move {
                world.ran(&tenant, ordinal, step)?;
                Ok(total.wrapping_add(weight))
            })
            .name(step)
            .await?;
        Ok(sum)
    }

    /// A step that makes one call to a peer. Restate retries the closure under the
    /// policy below, and journals the value it brought back.
    async fn called(
        &self,
        ctx: &WorkflowContext<'_>,
        (tenant, ordinal): (&str, u32),
        step: &'static str,
        (total, call): (u64, Call),
    ) -> Result<u64, HandlerError> {
        let (world, tenant) = (Arc::clone(&self.world), tenant.to_owned());
        let policy = RunRetryPolicy::new()
            .initial_delay(BACKOFF)
            .exponentiation_factor(2.0)
            .max_attempts(ATTEMPTS);
        let sum = ctx
            .run(|| async move {
                let value = world.call(call, &tenant, ordinal, step).await?;
                Ok(total.wrapping_add(value))
            })
            .retry_policy(policy)
            .name(step)
            .await?;
        Ok(sum)
    }

    /// The ten steps of run `ordinal`; the value is the running total.
    async fn ten_steps(
        &self,
        ctx: &WorkflowContext<'_>,
        tenant: &str,
        ordinal: u32,
    ) -> Result<u64, HandlerError> {
        let who = (tenant, ordinal);
        let [w1, w2, w4, w5, w7, w9, w10] = STEP_WEIGHTS;
        let mut total = u64::from(ordinal);
        total = self.local(ctx, who, "s1-intake", (total, w1)).await?;
        total = self.local(ctx, who, "s2-validate", (total, w2)).await?;
        total = self.called(ctx, who, "s3-outside", (total, Call::Outside)).await?;
        total = self.local(ctx, who, "s4-merge", (total, w4)).await?;
        total = self.local(ctx, who, "s5-prompt", (total, w5)).await?;
        total = self.called(ctx, who, "s6-model", (total, Call::Model)).await?;
        total = self.local(ctx, who, "s7-check", (total, w7)).await?;
        self.world.parked(ctx.key())?;
        let approved: u64 = ctx.promise(APPROVED).await?;
        if approved != u64::from(ordinal) {
            lgwks_std::trace::debug!(approved, ordinal, "the approval carries another run's number");
            return Err(TerminalError::new("the approval is for another run").into());
        }
        let held = u64::try_from(approved.to_le_bytes().len())?;
        total = total.wrapping_add(held);
        total = self.local(ctx, who, "s9-apply", (total, w9)).await?;
        self.local(ctx, who, "s10-finish", (total, w10)).await
    }
}

#[restate_sdk::workflow]
impl DurableFlow {
    /// The flow itself: runs once per key, and is told its own run number.
    #[handler]
    async fn run(&self, ctx: WorkflowContext<'_>, run: u32) -> Result<u64, HandlerError> {
        let Some((tenant, ordinal)) = parse_key(ctx.key()).filter(|key| key.1 == run) else {
            lgwks_std::trace::debug!(key = ctx.key(), run, "the workflow key is not <tenant>-<run> for this run");
            return Err(TerminalError::new("the key is not <tenant>-<run>").into());
        };
        let outcome = self.ten_steps(&ctx, tenant, ordinal).await;
        if outcome.is_err() {
            self.world.lost(ctx.key())?;
        }
        outcome
    }

    /// The decision: it grants the approval the run waits on, with the run's own
    /// number as the payload.
    #[handler]
    async fn approve(
        &self,
        ctx: SharedWorkflowContext<'_>,
        payload: u64,
    ) -> Result<(), HandlerError> {
        ctx.resolve_promise::<u64>(APPROVED, payload);
        Ok(())
    }
}
