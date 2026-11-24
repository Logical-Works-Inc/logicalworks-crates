---
name: lgwks-use-the-crates
description: Use when writing Rust that consumes these crates, here or in another repository, and you need the right front door. Routes a need to the one skill for script flows, durable runs, processes, serving, the kernel, tests or the rails, or to lgwks_std, lgwks_ast and the lgwks_deps storefront.
---

# Using the crates

Reach for the first row that fits, and never for the support code beneath it. The runtime makes the small calls (spawn, retry, deadline, temp dir, cleanup, tenant), so the skill for your job is the one to load.

| You need | Load | Read |
|---|---|---|
| Fan-out, deadlines, retries, steps, sagas, an untrusted plan | `skills/lgwks-script-flows/SKILL.md` | `docs/script-lexicon.md`, `crates/lgwks-bot/README-script.md` |
| A flow that survives a restart, runs once per key, or waits for approval | `skills/lgwks-durable-runs/SKILL.md` | `crates/lgwks-bot/src/task.rs` |
| A child process, its deadline, its temp dir, its confinement | `skills/lgwks-supervise-processes/SKILL.md` | `docs/adr/0077-supervised-children-get-a-private-temp-dir.md` |
| An HTTP edge, a per-tenant app, or `lgwks new`, `check`, `expand`, `run` | `skills/lgwks-serve-an-app/SKILL.md` | `crates/lgwks-bot/src/app.rs` |
| A journal, broker, lease, gate, effect or clock under your own runtime | `skills/lgwks-kernel/SKILL.md` | `docs/guides/lgwks-bot/kernel.md` |
| A test, a simulation, or a test the census failed | `skills/lgwks-test-on-the-rails/SKILL.md` | `.config/nextest.toml` |
| A temptation to hand-roll a spawn, retry, channel or dependency | `skills/lgwks-stay-on-the-rails/SKILL.md` | `skills/lgwks-std-first/SKILL.md` |
| Hex, base64, ids, time, hashing, JSON, HTTP, logging, with no async runtime | `lgwks_std` | `docs/guides/lgwks-std/index.md`, `docs/catalog.md` |
| One AST across languages, or a scan of Rust source | `lgwks_ast` | `docs/guides/lgwks-ast/index.md` |
| A third-party stack such as `tokio`, `axum`, `rmcp` or `candle` | The `lgwks_deps` storefront | `docs/guides/lgwks-deps/storefront.md` |
| Proof that no unowned dependency entered a build | `lgwks-deps check .` | `docs/guides/lgwks-deps/index.md` |

Look a capability up rather than guessing its owner:

```sh
cargo run --locked -p lgwks_deps --bin catalog -- --json .
```

## Install

```sh
cargo add lgwks_bot
cargo add lgwks_std
cargo install lgwks_deps
```
The package is `lgwks_bot`, and `lgwks-bot` is not a package. The defaults are `script`, `ecs`, `trace`, `wire` and `ron`. A capability is off by default where its cost or its target says so: `lgwks_bot` leaves out `net`, `process`, `fs`, `edge`, `signal`, `inspect`, `ephemeral` and `sealed`. The feature tables are in `docs/guides/lgwks-bot/features.md` and `docs/guides/lgwks-std/index.md`.

## Rules that save a rewrite

- A concurrency number typed as a literal is a compile error. A limit from outside is written `at most (quota) at once`.
- Retrying is decided by the error's variant: `FlowError::transient` retries and a permanent error never does. Do not match on a message.
- Untrusted model or tool output enters only through `admit`, which returns a typed plan or a typed refusal.
- A flow's `Tenant` is its host's partition key, not an authenticated identity.
- Capability gating is auditable authority, not a sandbox, because in-process code can always dial out.
- Deadlines and stops are cooperative and take effect at an `.await`.
- There is no `#[tokio::main]`. Entry is `lgwks_bot::script::block_on`.
- Do not add `tokio`, `serde` or `regex` as direct edges. `skills/lgwks-std-first/SKILL.md` maps each to its owner.
- A derive on a JSON type needs `#[serde(crate = "lgwks_std::json::serde")]`.

## What is promised

Only `script!`, `try_join!`, `domains!`, the `script` module, `lgwks_bot::kernel` and `lgwks_bot::app` are named at the root of `lgwks_bot`. Every other module is `#[doc(hidden)]` with no promise, so a consumer naming one pins an exact version. The process and supervise APIs are among them. `docs/guides/index.md` lists the surfaces of the other crates.

A gap in these crates is an issue to file, not a private copy to keep.
