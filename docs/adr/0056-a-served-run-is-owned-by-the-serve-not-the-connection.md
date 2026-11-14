# ADR 0056: a served run is owned by the serve, not the connection

Status: **accepted**, 2026-11-14. Implements the first step of #1008, from #823.

Stakeholders: the Director; consumers that serve many tenants through `rt::serve`; `lgwks_bot` maintainers.
Concerns: a thread hop and a private runtime for every request; a client that hangs up and poisons its tenant's store; a drain that leaves a run behind; the same answer for a retried key.
Requirements: R9, R10
Viewpoint: the request viewpoint, following one request from the axum fallback to the tenant's run store and back.
Elements and relations: `rt::serve::ingress`, `rt::serve::runs::Runs`, `Shared::runs`, `Host::submit`, `RunRecords::append_send`, `Records::record_send`, `RunStore::commit_send`.

## Context

`rt::serve` ran every request on the blocking pool, on a private current-thread runtime, because `Host::submit` was not `Send`. The first S1 profile of #823 shows the cost: context switches and futex wakes on every request, and a pool that the stores of other tenants also wanted. The submit future was not `Send` for one reason: the store door `RunRecords::append_async` returns a boxed, non-`Send` future (`crate::BoxFuture`) that `Records::claim` and `Records::record` awaited.

Dropping the hop has a catch. A run awaits its append through `Awaiting`, and dropping that future before the owner answers marks the request abandoned and poisons the tenant's store handle until a reopen (`journal/owner/store.rs`, INV-BOT-15). An axum handler future is dropped when its client hangs up. If the run lived in that future, one hang-up could poison a tenant.

## Decision

1. **The submit future is `Send`.** `RunRecords` gains one provided method, `append_send`, which returns a boxed `Send` future. Its default calls `append` on the awaiting thread, as the `append_async` default does. `RunStore` overrides it with the owner's concrete `Awaiting`. `Records::claim` and the new `Records::record_send` use it, and `Host::submit` awaits those two.
2. **Each admitted run is a task in a set the serve owns.** `ingress` builds an `Admitted` value holding the host, handler, key, body, in-flight permit and live guard, and spawns its `drive` into `Runs`. The handler awaits a `oneshot` for the outcome. A client that leaves drops the receiver and nothing else: the run finishes and records its reply.
3. **The drain owns the set.** Once the server task ends, the drain takes the set and joins it for the drain grace. Past the deadline the hosts are cancelled, the set gets one second to answer, and the rest are aborted. Each aborted run counts as cancelled in the `ServeReport`.
4. **The in-flight ceiling bounds the set.** The permit is owned by the run, so the set never holds more tasks than `max_in_flight`, at most 65,536. Past it the edge answers `503` with `Retry-After`, as before.
5. **A handler runs on a driver worker.** It must not block the thread. `TypedRoute::new` says so.

## Alternatives rejected

- **Await `Host::submit` inline in the handler.** It needs no set and no lock. It lets a hang-up poison a tenant's store.
- **Keep the pool hop.** It is correct and slow, and it takes a pool thread per request.
- **Change `append_async` to return a `Send` future.** It breaks every external `RunRecords` implementation that overrides it.
- **A bare `tokio::spawn`.** `rt/task.rs` forbids a spawn that nothing owns.

## Consequences

- A store that overrides `append_async` and not `append_send` blocks the awaiting thread on the request-key door. The host's own `RunStore` overrides both.
- A handler that blocks its thread now holds a driver worker, where it held a pool thread before.
- `Runs::spawn` takes one short `std` mutex per request, to spawn and to reap finished tasks.
- The set of tenant hosts is still one global mutex. ADR 0056 is extended when that changes.
- Measured on the 2-CPU 2 GB Linux box, release build, `taskset -c 0,1`, 2 GiB cap, `edge-load --rig --only <surface>`, before (c0cd3aa0) then after, with other sessions loading the box (load 11 to 39):

| surface | conns | rps before | rps after | p99 us before | p99 us after |
|---|---|---|---|---|---|
| serve | 64 | 5122 | 5076 | 20683 | 20084 |
| serve | 1024 | 7070 | 6898 | 219479 | 178023 |
| serve-durable | 64 | 4753 | 5159, 5815 | 22771 | 26307, 20274 |
| serve-durable | 1024 | 6481 | 7099, 7150 | 223791 | 182878, 178460 |

- Throughput is flat within the load noise: the two cores are spent on the store and the receipt hash, not on the hop. The tail at 1024 connections falls by about a fifth, and no row's errors moved from zero.

## Correspondence

Relates to ADR 0006, which puts one driver on tokio and axum, and to ADR 0013, which describes the serve edge. It touches INV-BOT-15 and INV-BOT-185.

## Verification

- `crates/lgwks-bot/tests/it/sim_serve_hop.rs`: `sim_serve_hop_matches_the_model`, `sim_serve_hop_same_seed_same_trace` and `sim_serve_hop_distinct_seeds_diverge` drive the shipped `serve` over loopback against a reference model. `a_client_that_leaves_mid_run_does_not_abandon_the_run` hangs up mid-run and reattaches the key.
- `crates/lgwks-bot/src/rt/serve/runs.rs`: `sim_drain_joins_every_seeded_run`, `sim_drain_reports_what_it_cancelled` and `a_stuck_run_is_aborted_past_the_grace`.
- INV-SERVE-1 in `crates/lgwks-bot/INVARIANTS-3.md`.
