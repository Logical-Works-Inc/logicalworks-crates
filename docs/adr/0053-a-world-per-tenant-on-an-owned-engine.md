# ADR 0053: a world per tenant on an owned engine

Status: **accepted**, 2026-11-14, for #1001 under #985.

Stakeholders: the Director; an app author on the host; the agent of each world leaf (W2 to W6).
Concerns: how a bot and a person pass information cheaply; Erlang-level fault tolerance per tenant; an admitted
engine that cannot hold a tenant's state.
Requirements: R1, R3, R7, R11
Viewpoint: the runtime-state viewpoint: boxes are tenant cells, worlds, logs and the cell host, and arrows are who
may read or write whose state.
Elements and relations: the tenant cell, its world, its log, its mailbox, the cell host, the owned engine,
`bevy_ecs`; relations: `owns`, `folds`, `appends`, `supervises`.

## Context

Read on main `37fbb264`, 2026-11-14.

- `bevy_ecs` is named by 14 production import lines, all under `ecs/`, for example
  `crates/lgwks-bot/src/ecs/fold.rs:3` and `crates/lgwks-bot/src/ecs/resources.rs:11`.
- The world holds two components, `SourceId` (`crates/lgwks-bot/src/ecs/resources.rs:37`) and `Revision`
  (`crates/lgwks-bot/src/ecs/resources.rs:59`).
- One schedule runs two exclusive systems in order (`crates/lgwks-bot/src/ecs/fold.rs:758`), `observe_fold` and
  `fire_plan`, each taking `&mut World` (`crates/lgwks-bot/src/ecs/fold.rs:67`). The executor is single-threaded
  on purpose (`crates/lgwks-bot/src/ecs/fold.rs:751`).
- Real state lives in resources indexed by chain position, not in components: a source's id is only "the key into
  `Chains` and `Observed`" (`crates/lgwks-bot/src/ecs/resources.rs:38`).
- The one query is `Changed<Revision>` (`crates/lgwks-bot/src/ecs/resources.rs:184`), copied into a `Vec<bool>`
  each tick (`crates/lgwks-bot/src/ecs/resources.rs:172`).
- The cell host checkpoints by cloning a tenant's state (`crates/lgwks-bot/src/cell.rs:543`). A bevy `World` is not
  `Clone` and has no wire form, so it cannot be that state.
- The cell host catches panics today (`crates/lgwks-bot/src/cell/host.rs:16`).
- `cargo tree --locked -p lgwks_bot -i bevy_ecs -e normal --depth 0` printed `bevy_ecs v0.20.1`. ADR 0005 named 0.19.

## Decision

1. **Each tenant owns one world.** A world is typed columns keyed by entity id, with a change tick per row. It is
   the private heap of that tenant's cell. Nothing outside the cell holds a reference into it.
2. **Cheap passing is inside a world.** A bot and a person in one tenant write components and read by query. One
   change subscription wakes the bot and pushes to the person's screen. No copy, no encode, no call between them.
3. **Isolation is between worlds.** Tenants share nothing. A command record crosses through a bounded mailbox. It
   is the bytes the log already holds, so crossing a cell or a host costs no second encoding.
4. **A system is a transaction.** A system reads the world and writes a change set. The log appends the change set,
   then the world applies it. A panic or a refusal drops the change set, and the world is byte-identical to before.
5. **The supervisor is the cell host.** Each world sits behind its own lock, taken only by its worker. A dead worker
   poisons only its lock. The host starts a worker and rebuilds only that tenant from checkpoint and log. Restarts
   count in a window; past the bound the tenant answers `503` and the host serves the rest. No `catch_unwind`.
6. **The engine is ours and small.** Sequential, one deterministic order, a wire form for every component, and a
   snapshot written as columns by change tick. No parallel scheduler: a cell is sequential by design.

This replaces one word of ADR 0006 decision 1: "one ECS world" becomes "one world per tenant, on one host".

| Erlang | Here |
|---|---|
| process | tenant cell |
| private heap | world |
| message | command record |
| link | thread join |
| supervisor | cell host |
| restart intensity | restarts per window |
| let it crash | dropped change set |

## Alternatives rejected

- **Keep `bevy_ecs`.** Its world is not `Clone` and has no wire form, so it cannot hold checkpointed state. Its
  strength is a parallel schedule the tick runs single-threaded. It is pre-1.0 and moved from 0.19 to 0.20.1.
- **Drop ECS for opaque cell state.** It isolates as well. No second reader can query it, it has no change feed, and
  it checkpoints by cloning everything. Every app would then build its own tables, indexes and subscriptions.
- **One shared world for all tenants.** Tenants share one heap, a panic reaches every tenant, and isolation becomes
  a filter every query must remember.

## Consequences

- W2 builds the world, W3 races it against `bevy_ecs`, W4 makes it the cell's state, W5 is the shared change feed,
  W6 is the supervisor. `bevy_ecs` leaves only when the owned world wins its matched tick (ADR 0005 decision 4).
- The call is wrong if any falsifier below holds:
  - W3: the owned world loses to `bevy_ecs` on the tick at 1,000, 10,000 or 100,000 sources, on p99 or peak RSS.
    Then the tick keeps `bevy_ecs` and the tenant world stays ours.
  - W4: a transaction commit has a higher p99 than the cell's clone-and-rebuild on the write route of the S1 rig
    (#1007), at 100 tenants and 1,024 connections.
  - A consumer measures one SQL file per tenant ahead of world plus log on its own write path, on p99 at the S1
    workload. Then that app keeps SQL and uses the world for agents and screens.
- Not decided: the query language. Cross-tenant reads do not exist; a report over many tenants is a fold of their logs.

## Correspondence

- Refines ADR 0006 decision 1 by the one word above. Decisions 2 to 8 of ADR 0006 stand.
- Builds on ADR 0005 decision 4, which already retires an admitted engine that loses a matched workload.

## Verification

- The `doc-governance` lane holds this record's fields. The `doc-citations` lane pins each cited line.
- `crates/lgwks-bot/tests/it/core_boundary.rs` holds decision 5 now: `a_new_catch_unwind_site_is_refused` refuses
  a new site, and the walk refuses a listed file that stopped catching. The list leaves with W6 (#1006).
- Decisions 1 to 4 and 6 are `review only` until W2 and W4 land, owner the agent of W4, dated 2026-11-14.
