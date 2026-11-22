# Choosing a crate

This page maps a need to a crate and a feature, and states the dependency graph.

**The core is not zero-dependency by default.** Only `--no-default-features
--features core` pulls nothing. The default set is `standard`: every capability
but `http`. The `lgwks_std` README says so too, and the
[repository quickstart](../README.md#quickstart) assumes the default set.

**The dependency graph.** `lgwks_std` depends on nothing in this workspace.
`lgwks_ast` depends on `lgwks_std` only for its `tool`. `lgwks_deps` depends on
`lgwks_std` and `lgwks_ast`. `lgwks_macros` depends on those three. `lgwks_bot`
depends on those four. `drishti` depends on `lgwks_bot`, `lgwks_std` and
`lgwks_deps`.

| I need... | Reach for | Feature |
|---|---|---|
| Hex, base64, timestamps, glob, LEB128 | `lgwks_std` core | default; `core` alone has zero deps |
| UUIDs, hashing | `lgwks_std::id`, `lgwks_std::hash` | `random`, `hash`, both default |
| Retry budgets (attempts, backoff, deadlines) | `lgwks_std::retry` | `core`, in the default |
| A seeded, replayable stream for deterministic simulations | `lgwks_std::seeded` | `core`, in the default |
| Edit-distance / Jaccard / cosine scoring, weighted composition | `lgwks_std::similarity` | `core`, in the default |
| Directory walking with depth, order and symlink policy | `lgwks_std::fs` | `core`, in the default |
| Regex, JSON, RON, binary wire, reachability | `lgwks_std` | `pattern`, `json`, `ron`, `wire`, `online`, all default |
| HTTP client | `lgwks_std::http` | `http`, or `full` |
| `block_on`, bounded `join_all`, `spawn_blocking` (sync) | `lgwks_std::task` | `core`, in the default |
| Orchestration: fan-out, deadlines, retries, steps | `lgwks_bot::script!` | default |
| Capability-gated automation (Observe/Evaluate/Execute/Query) | `lgwks_bot` | default |
| A journal, clock, broker or lease under your own runtime | `lgwks_bot::kernel` | default; see [the kernel guide](guides/lgwks-bot/kernel.md) |
| Async timers, channels, sockets, processes, signals | `lgwks_bot::rt` | `rt`, `time`, `sync` by default; `net`, `process`, `fs`, `signal` by name |
| Parse any of 28 languages in a tool | `lgwks_ast` | default |
| Rust source lint scan (zero-gate detectors) | `lgwks_ast` | `scan` and `tool`, both default, run as `lgwks-ast scan` |
| Raw `tokio` engine without the bot facade | `lgwks_deps` | `tokio*` with `default-features = false` |
| GPU desktop UI (Zed's GPUI) | `lgwks_deps` | `gpui` with `default-features = false` |
| Native terminal UI (AppCUI) | `lgwks_deps` | `appcui` with `default-features = false`, version as `contract/APPROVED.toml` pins it |
| Detached ed25519 seals that verify offline from the public key | `lgwks_std::sign` | `sign`, default (ed25519-dalek, alloc-only, no rand_core) |

Know three things before you depend on them:

- `tempfile` is a development-only edge. There is no production temporary-directory path.
- Typed errors derive through `lgwks_macros` (`#[derive(lgwks_macros::Error)]`), not `lgwks_std`.
- `join!`, `select!` and `try_join!` in `lgwks_bot` need the `macros` feature. There is no `#[tokio::main]`: enter through `Runtime::block_on`.

## The assembled runtime

`lgwks_bot::rt::estate::Estate` (the `script` feature) is one tenant behind one
call. It owns the runtime, the supervisor and the host behind three tiers:
`in_process`, `supervised` and `durable`. `durable` also needs the `ephemeral`
feature.

- Run work with `run_task` or `run_blocking`.
- Drain to one terminal report with `shutdown` or `shutdown_blocking`.
- Reach what the estate owns with `host`, `supervisor` and `handle`. Supervised work is counted on the report.

`tests/it/estate.rs` runs every door, including a drop and a refused build inside
a runtime. `tests/it/sim_estate.rs` holds the ceiling at 100, 1,000 and 10,000
runs. `src/bin/estate.rs` runs the same evening as a smoke that a test runs.
