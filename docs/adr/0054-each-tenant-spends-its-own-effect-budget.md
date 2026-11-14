# ADR 0054: each tenant spends its own effect budget

Status: **accepted**, 2026-11-14. Implements part of #1042, from #1044.

Stakeholders: the Director; consumers that serve many tenants on one `App`, first Larry (larry#210); `lgwks_bot` maintainers.
Concerns: one tenant spending a shared provider or command pool for all; a retry loop that costs nothing; a flood of tenant names growing a table without bound; a verdict that replays differently.
Requirements: R9, R10
Viewpoint: the effect viewpoint, mapping each effect a handler returns to the tenant that pays for it.
Elements and relations: `App::budget`, `BudgetLimits`, the `app.budget` cap a refusal names, `AppError::BadBudget`; the `app::budget` ledger beside the runner; `App::check`.

## Context

An `App` had no per-tenant limit on the effects it serves. `CommandLimits` bounds commands only, and the edge bounds are the same for every tenant. One tenant could spend the provider and the command pool for all of them. #1042 asks for a per-tenant rate through `App`, with today's behaviour as the default.

## Decision

1. **A budget is opt-in on the builder.** `App::budget(BudgetLimits, Clock)` sets an allowance per window and the most tenants tracked. An app with no budget serves as before.
2. **Each served effect costs one unit of its tenant's budget.** An `Ask` or a `Command` is charged whether or not the provider answers, so a retry pays for each try. An effect refused for a missing cap is not charged.
3. **Past the budget, the answer is a refusal.** The effect is answered `EffectResult::Refused` naming `app.budget` and never reaches the provider or the pool. An `on` handler reads it as it reads a missing cap.
4. **The window is fixed and per tenant.** It opens at the tenant's first charge, on the injected clock. The state is whole integers, so a replay gives the same verdicts.
5. **The table is bounded.** It holds at most `tenants` windows. When it is full, ended windows go. If it is still full, the new tenant is refused, and no live window is evicted.
6. **`check` refuses a budget that cannot run.** A zero window or zero tenants is `AppError::BadBudget`.

## Alternatives rejected

- **A token bucket.** It needs fractional refill, so its verdicts depend on rounding. The fixed window is exact.
- **A sliding log.** It keeps one entry per unit, which a tenant can grow to its allowance.
- **One global allowance.** A noisy tenant would spend it and starve its neighbours.
- **Evicting the oldest window when full.** A flood of new names would reset a live tenant's spend.

## Consequences

- A tenant can spend its allowance at the end of one window and again at the start of the next, so a burst can reach twice the allowance.
- The ledger is one mutex, held for a map lookup and a few integer steps per effect. Its cost on the `app` rig rows is not measured yet.
- TLS and per-tenant body and concurrency limits stay open on #1042.

## Correspondence

Relates to ADR 0025, whose commands the budget charges, and to the IETF RateLimit header fields draft, whose quota policy is `q` units per window `w` per partition key.

## Verification

- `crates/lgwks-bot/src/app/sim_budget.rs`: `sim_budget_same_seed_replays_the_same_trace_hash` drives the real host against a model. It checks every answer and that no refused ask reached the provider.
- The same file holds the window edge, the full table, a name that is no tenant, `check`, and a command past the budget.
- INV-BUDGET-1 in `crates/lgwks-bot/INVARIANTS-3.md`.
