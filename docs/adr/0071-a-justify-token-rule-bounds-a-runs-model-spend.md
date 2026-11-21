# ADR 0071: a `justify` token rule bounds a run's model spend

Status: **accepted**, 2026-11-20.

Stakeholders: the Director; a tenant who pays per token; an operator who sets a run's spend ceiling.
Concerns: nothing bounded what a run's `ask` steps spent. A `justify` block governed `observe`, `act` and every
retry, but not `ask`, so a flow that looped on a model answer had no ceiling but the provider's bill.
Requirements: INV-BOT-199, INV-BOT-198, INV-BOT-184.
Viewpoint: one run's model spend, as its `ask` steps reach the provider.
Elements and relations: `script::justify::{JustifyCtx, Rule::tokens, Allowance}`, `script::ask`, `ModelOutput`.

## Context

Pydantic AI's `UsageLimits` checks a run's request and token counts against the provider's reported usage, and
refuses the next request past the limit. OpenAI's Agents SDK bounds turns, not tokens. The estate already had the
mechanism: `justify` evaluates ordered, deterministic rules before effectful steps, and `Allowance` is a refillable,
lock-free counter. It had no unit of cost, and `ask` never consulted it.

## Decision

1. **`JustifyCtx` carries a `cost`.** An `ask` names its `max_tokens`; every other step names zero.
2. **`Rule::tokens` reserves the whole cost from an `Allowance` before the step, or blocks and spends nothing.**
   The reservation is one compare-and-swap, so asks racing on one allowance cannot together pass it.
3. **The reservation settles to the provider's bill.** `ModelOutput::with_billed` reports it, and an under-bill
   refunds. An over-bill (prompt tokens) spends the excess, floored at zero. No bill keeps the whole reservation.
4. **A failed or cancelled call keeps its reservation.** A provider may bill a call it never answered.
5. **The rules are asked inside the durable step.** A replayed `ask` neither asks them nor spends.

## Alternatives rejected

- **A second token ledger on the host `Policy`.** It would be a second budget mechanism beside `Allowance`, and
  `justify` would still not govern `ask`. Reverse this if a measured host-wide ceiling is needed that a per-run
  rule cannot express.
- **Refund a cancelled reservation.** The run cannot know whether the provider billed it; a refund it cannot see
  lets a fan-out spend past the allowance.

## Consequences

- A run under `justify with (rules)` holding `Rule::tokens` is refused `FlowError::Justified` before the model is
  called once its allowance cannot cover the next budget.
- Cancelled asks over-charge the allowance; the error is always toward spending less.

## Correspondence

Refines ADR 0070 (the durable `ask`) and #657 (justification).

## Verification

`crates/lgwks-bot/tests/it/sim_ask_replay.rs`: `sim_a_token_budget_stops_the_ask_before_the_model`,
`sim_concurrent_asks_never_spend_past_the_budget` and `sim_a_replayed_ask_spends_no_tokens`;
`script::justify::tests::a_token_rule_spends_the_cost_and_settles_to_the_bill`.
