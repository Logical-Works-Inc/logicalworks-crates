# lgwks_bot

**Resilient workflow orchestration & durable execution engine for Rust: indented `script!` flows with bounds the macro enforces, shared retry budgets, values a resume reads back, and native admission barriers for AI agent plans.**

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

## The 8 Production Traps Ruled Out by Design

| Block | What it structurally rules out |
|---|---|
| `each x in xs:` | **Unbounded fan-out & leaks**: Concurrency is bounded; if an item fails, siblings abort cleanly. |
| `within 2s:` | **Silent hangs & zombie tasks**: A deadline on the scope's clock; cancellation propagates down the tree. |
| `retry up to 3 times, waiting 100ms:` | **Retry storms**: Unified attempt budget with jitter; permanent errors are never retried. |
| `together:` | **Unstructured concurrency**: Replaces `tokio::spawn` with structured branches. |
| `step name:` | **Anonymous failures**: A named scope with its own key and error location. |
| `remember name:` | **Lost work on a crash**: The block runs once per run; a resume reads its value back. |
| `ready x from (readiness) within 30s:` | **Sleep-based readiness guesses**: Readiness probes instead of sleep loops. |
| `admit from model through (gate):` | **Untrusted AI execution**: Gated admission ensures untrusted LLM outputs cannot execute without validation. |

For an architectural comparison against Temporal and raw Tokio, see [Durable Execution in Rust](https://github.com/Logical-Works-Inc/logicalworks-crates/blob/HEAD/docs/comparison-vs-temporal-tokio.md).

## Quickstart

A complete program, the one `published-consumer` builds from crates.io:
```rust
use lgwks_bot::script::{FlowError, Scope, Tenant};

lgwks_bot::script! {
    /// Square every number, as many at once as the machine sustains; each
    /// attempt gets a second and a failure worth retrying is retried under
    /// the same key.
    pub flow squares(numbers: Vec<u64>) -> u64:
        let squared = each number in numbers:
            retry up to 3 times, waiting 10ms:
                within 1s:
                    number.saturating_mul(number)
        give back squared.iter().sum()
}

fn main() -> Result<(), FlowError> {
    let scope = Scope::root(Tenant::new("acme")?);
    let total = lgwks_bot::block_on(squares(&scope, vec![1, 2, 3]))?;
    assert_eq!(total, 14);
    assert!(ARCHITECTURE.to_string().contains("each number in numbers"));
    Ok(())
}
```
The supported interface is `script!`, `try_join!`, the `script` module and, for a host of flows, `lgwks_bot::kernel`. [README-script.md](README-script.md) explains the flow above; [docs.rs/lgwks_bot](https://docs.rs/lgwks_bot) has runnable examples.

## Features

| Feature | Default | Enables |
|---|---|---|
| `rt`, `time`, `sync`, `macros` | on | the orchestration runtime surface, driven through `lgwks_deps` |
| `script` | on | `script!` and the `script` module |
| `ecs`, `trace`, `wire`, `ron` | on | the ECS substrate, the trace macros and the encodings the journal persists through |
| `io`, `net`, `fs`, `process`, `signal`, `edge` | off | the matching runtime drivers, and the HTTP edge on axum |
| `inspect` | off | structural code inspection through `lgwks_ast` |
| `ephemeral`, `profile`, `testing` | off | run identities from `lgwks_std::random`; the per-stage profiler; the run-store conformance suite |
| `sealed`, `legacy` | off | records sealed at rest; the `lgwks_std` legacy codecs |
| `full` | off | every feature above |

## Architecture: one ECS world, one driver

[ADR 0006](https://github.com/Logical-Works-Inc/logicalworks-crates/blob/HEAD/docs/adr/0006-one-ecs-world-one-tokio-axum-driver.md)
fixes the shape. One ECS world is the core: a source moving, a run admitted or
settled, a timer firing is an event written to it, and effects leave through
the journal. One driver on tokio and axum sits below the world, used only where
rebuilding the piece makes no sense, and a consumer never names it: it is owned
through the `lgwks_deps` storefront. Above the world sits `script!`, the
framework an agent or a developer writes in.

The boundary is a test, not a convention: `tests/it/core_boundary.rs` fails the
build if a core module names the driver or a file outside `ecs/` names bevy.

This is **not a sandbox**. In-process code can always dial out directly, and
capability gating is auditable authority, not isolation. A flow's `Tenant` is
its trusted host's partition key rather than an authenticated identity.

## Stability

Everything outside `script!`, `try_join!`, the `script` module and `lgwks_bot::kernel`
is `#[doc(hidden)]` support code with no compatibility promise. Which release
carries a symbol is in [`CHANGELOG.md`](../../CHANGELOG.md), with the 3.0.0
migration notes under [`docs/changelog/`](../../docs/changelog). The minimum
supported Rust version is 1.98.

## Boundaries and failure

Async-client limits are in [distributed-mesh boundaries](../../docs/distributed-boundaries.md); background bounds in [bounded background work](../../docs/guides/lgwks-bot/background-work.md); tick failures in [what a failed tick means](../../docs/guides/lgwks-bot/failures.md).

## Publisher

Logical Works Incorporated publishes `lgwks_bot` as one of six independently versioned crates in [logicalworks-crates](https://github.com/Logical-Works-Inc/logicalworks-crates#readme): `lgwks_std`, `lgwks_bot`, `lgwks_ast`, `lgwks_macros`, `lgwks_deps` and `drishti`.
[Should I use logicalworks-crates?](https://github.com/Logical-Works-Inc/logicalworks-crates/blob/HEAD/docs/should-i-use.md) says when to reach for each one and when to skip it.

## License

MPL-2.0 — Copyright 2026 Logical Works Incorporated

Deliberately not the workspace's Apache-2.0. The bot is the artefact the rest of the estate embeds, so it carries file-level copyleft while its siblings stay permissive.
A proprietary consumer is still permitted — MPL-2.0 §3.3 — and only modification of the MPL-covered files carries an obligation. See [`LICENSING.md`](../../LICENSING.md).
