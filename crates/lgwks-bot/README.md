# lgwks_bot

**Resilient workflow orchestration & durable execution engine for Rust: indented `script!` flows with compiler-guaranteed bounds, shared retry budgets, crash-safe step checkpoints, and native admission barriers for AI agent plans.**

Pages of this README: [overview](README.md) · [execution model](README-execution.md) · [failure](README-failure.md) · [authority, domains and specs](README-authority.md) · [script!](README-script.md) · [async runtime](README-runtime.md) · [supervision](README-supervision.md)

[![crates.io](https://img.shields.io/crates/v/lgwks_bot.svg)](https://crates.io/crates/lgwks_bot)
[![docs.rs](https://img.shields.io/docsrs/lgwks_bot)](https://docs.rs/lgwks_bot)

Install via Cargo:
```sh
cargo add lgwks_bot
```

## Why lgwks_bot?

Writing resilient async logic in Rust usually forces a bad compromise:
- **Raw Tokio**: Hand-rolling `select!`, semaphore permits, and custom retry loops prone to leaked futures, zombie tasks, and retry storms.
- **Distributed Workflow Engines (Temporal)**: Great durability, but requires operating external server clusters, databases, and managing fragile event-history replay compatibility across versions.

`lgwks_bot` embeds enterprise durable workflow orchestration directly inside your Rust process. Zero external servers required.

## The 7 Production Traps Ruled Out by Design

| Block | What it structurally rules out |
|---|---|
| `each x in xs:` | **Unbounded fan-out & leaks**: Concurrency is bounded; if an item fails, siblings abort cleanly. |
| `within 2s:` | **Silent hangs & zombie tasks**: Mandatory deadlines; cancellation propagates down the tree. |
| `retry up to 3 times, waiting 100ms:` | **Retry storms**: Unified attempt budget with jitter; permanent errors are never retried. |
| `together:` | **Unstructured concurrency**: Replaces `tokio::spawn` with structured branches. |
| `step name:` | **Anonymous crashes**: Deterministic keys; state checkpoints enable crash resumption without replay drift. |
| `await ready(&db, 30s)` | **Sleep-based readiness guesses**: Deterministic readiness probes instead of sleep loops. |
| `admit the model's plan:` | **Untrusted AI execution**: Gated admission ensures untrusted LLM outputs cannot execute without validation. |

For architectural comparisons and benchmarks against Temporal and raw Tokio, see [Durable Execution in Rust](https://github.com/srinji-kaggss/logicalworks-crates/blob/HEAD/docs/comparison-vs-temporal-tokio.md).

## Quickstart

Flow in 12 lines:
```rust
use lgwks_bot::script::{FlowError, Scope, Tenant};

lgwks_bot::script! {
    /// Resilient, bounded, tenant-partitioned pipeline in 12 lines
    flow sync_records(client: &ApiClient, db: &Database, records: Vec<Record>) -> SyncReport:
        step validate:
            admit records.iter().all(|r| r.is_valid())?
        step upload:
            let receipts = each record in records:
                retry up to 3 times, waiting 50ms:
                    within 5s:
                        client.upload(&record).await?
        step commit:
            db.record_sync(&receipts).await?
        give back SyncReport::success(receipts.len())
}
```

The supported interface is `script!`, `try_join!` and the `script` module. [README-script.md](README-script.md) has a complete flow; [docs.rs/lgwks_bot](https://docs.rs/lgwks_bot) has runnable examples.

## Features

| Feature | Default | Enables |
|---|---|---|
| `rt`, `time`, `sync`, `macros` | on | the orchestration runtime surface, driven through `lgwks_deps` |
| `script` | on | `script!` and the `script` module |
| `io`, `net`, `fs`, `process`, `signal` | off | the matching runtime drivers |
| `inspect` | off | structural code inspection through `lgwks_ast` |
| `ephemeral`, `profile` | off | run identities from `lgwks_std::random`; the per-stage profiler |
| `full` | off | every feature above |

## Architecture: one ECS world, one driver

[ADR 0006](https://github.com/srinji-kaggss/logicalworks-crates/blob/HEAD/docs/adr/0006-one-ecs-world-one-tokio-axum-driver.md)
fixes the shape. One ECS world is the core: a source moving, a run admitted or
settled, a timer firing is an event written to it, and effects leave through
the journal. One driver on tokio and axum sits below the world, used only where
rebuilding the piece makes no sense, and a consumer never names it: it is owned
through the `lgwks_deps` storefront. `script!` is the framework above the world
that an agent or a developer writes in.

The boundary is a test, not a convention: `tests/it/core_boundary.rs` fails the
build if a core module names the driver or a file outside `ecs/` names bevy.
The `Estate`, `Host` and `Supervisor` still build their own runtimes in 4.0.0;
ADR 0006 R2 and R3 move them onto the one driver and into the world.

This is **not a sandbox**. In-process code can always dial out directly, and
capability gating is auditable authority, not isolation. A flow's `Tenant` is
its trusted host's partition key rather than an authenticated identity.

## Stability

Everything outside `script!`, `try_join!` and the `script` module is
`#[doc(hidden)]` support code with no compatibility promise. Which release
carries a symbol is in [`CHANGELOG.md`](../../CHANGELOG.md), with the 3.0.0
migration notes under [`docs/changelog/`](../../docs/changelog). The minimum
supported Rust version is 1.98.

## Boundaries and failure

Async-client limits are in [distributed-mesh boundaries](../../docs/distributed-boundaries.md); background bounds in [bounded background work](../../docs/guides/lgwks-bot/background-work.md); tick failures in [what a failed tick means](../../docs/guides/lgwks-bot/failures.md).

## Publisher

Logical Works Incorporated publishes `lgwks_bot` as one of six independently versioned crates in [logicalworks-crates](https://github.com/srinji-kaggss/logicalworks-crates#readme): `lgwks_std`, `lgwks_bot`, `lgwks_ast`, `lgwks_macros`, `lgwks_deps` and `drishti`.
[Should I use logicalworks-crates?](https://github.com/srinji-kaggss/logicalworks-crates/blob/HEAD/docs/should-i-use.md) says when to reach for each one and when to skip it.

## License

MPL-2.0 — Copyright 2026 Logical Works Incorporated

Deliberately not the workspace's Apache-2.0. The bot is the artefact the rest of
the estate embeds, so it carries file-level copyleft while its siblings stay
permissive. A proprietary consumer is still permitted — MPL-2.0 §3.3 — and
only modification of the MPL-covered files carries an obligation. See
[`LICENSING.md`](../../LICENSING.md).
