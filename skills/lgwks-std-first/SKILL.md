---
name: lgwks-std-first
description: Use before adding any dependency to a LogicalWorks Rust crate, or when a task suggests tokio, futures, async-trait, pollster, syn, proc-macro2, regex, uuid, chrono, walkdir, glob, base64, hex, percent-encoding, serde_json, ureq, reqwest, ast-grep, once_cell, or num_cpus. Maps each crate to its std / lgwks_std / lgwks_bot / lgwks_ast replacement, gives the storefront and syn boundaries, and routes genuinely missing capability through lgwks-dependency-admission.
---

# std-first: the workspace replacement map

This workspace compiles against `std` and `lgwks_std` first, then `lgwks_bot` (async,
runners, actors), and `lgwks_ast`; everything else is a registered BOUNDARY
feature of the `lgwks_deps` storefront. Reaching for a familiar crate when a
first-party module exists is the most common agent defect in this repo, and
`lgwks-deps check` refuses it. Full rationale and boundaries:
`docs/dependency-doctrine.md`.

## Before `cargo add`, answer these

1. Is it in `std`? (`OnceLock`/`LazyLock`, `available_parallelism`,
   `mpsc`, `thread`, `fs`, `net`, `from_le_bytes`.)
2. Is it a workspace module? (table below, and `docs/catalog.md`.)
3. Is it parsing? Every parser is `lgwks_ast`, including the Rust scan
   (`lgwks_ast::scan`). Do not add a second parser.
4. Is it async? `lgwks_bot` for the runtime, runners, and actors;
   `lgwks_std::task` for the zero-dep synchronous subset.
5. Still missing? `skills/lgwks-dependency-admission/SKILL.md`.

## Replacement map

Import the right-hand side, not the crate.

| Instead of | Import | Feature |
|---|---|---|
| `hex` | `lgwks_std::hex` | core |
| `base64` | `lgwks_std::encoding::base64` | core |
| `percent-encoding` | `lgwks_std::encoding::percent` | core |
| `uuid` | `lgwks_std::id::Uuid` | `random` |
| `getrandom`, `rand` entropy | `lgwks_std::random` | `random` |
| `chrono`, `time` | `lgwks_std::time` | core |
| `glob` | `lgwks_std::glob` | core |
| `walkdir` | `lgwks_std::fs::walk_dir` | core |
| `fs4`, `statvfs` | `lgwks_std::fs::available_space` | `fs-raw` |
| `regex` | `lgwks_std::pattern::Regex` | `pattern` |
| `blake3` | `lgwks_std::hash` | `hash` |
| `ed25519-dalek` | `lgwks_std::sign` | `sign` |
| `serde` | `lgwks_std::json::{Serialize, Deserialize}` | `json` |
| `serde_json` | `lgwks_std::json` | `json` |
| `ron` | `lgwks_std::ron` | `ron` |
| `rkyv` | `lgwks_std::wire` | `wire` |
| `ureq`, `reqwest` (blocking) | `lgwks_std::http` | `http` |
| `url`, `iri-string` (validate) | `lgwks_std::http::validate_url` | `http` |
| `tokio` | `lgwks_bot` through the `lgwks_deps` storefront | `rt` |
| `tokio::time`/`sync`/`net`/`process`/`fs`/`signal` | `lgwks_bot::rt::{time, sync, net, process, fs, signal}` | matching bot feature |
| `futures`, `pollster`, `async-trait`, sync-only `tokio` | `lgwks_std::task` | core |
| actor/state frameworks | `lgwks_bot` | — |
| `tree-sitter`, `ast-grep-*` | `lgwks_ast` | — |
| `syn`, `proc-macro2` for reading source | `lgwks_ast::scan` | `scan` |
| `thiserror` | `#[derive(lgwks_macros::Error)]`, or `lgwks_ast::define_error!` | — |
| admission/audit tooling | `lgwks_deps` | — |
| `once_cell`, `lazy_static` | `std::sync::{OnceLock, LazyLock}` | — |
| `num_cpus` | `std::thread::available_parallelism` | — |
| `bytes`, `byteorder` | slices + `from_le_bytes` | — |

`lgwks_std`'s default is `standard`, every capability but `http`; only
`default-features = false, features = ["core"]` has zero dependencies. Each feature
pulls one vetted stack, tabled in `docs/guides/lgwks-std/index.md`.

**serde derive:** do not add `serde` as a direct dependency, because the gate refuses
it (`allowed_consumers = lgwks_std`). Re-export and annotate instead:

```rust
use lgwks_std::json;
#[derive(json::Serialize, json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde")]
struct Doc { n: u32 }
```
## tokio: two tiers, one owned engine

**Synchronous (`lgwks_std::task`, zero deps):** `block_on`, `join_all`, and
`spawn_blocking` replace `futures`, `pollster`, and `async-trait` for code that
only awaits futures and keeps blocking work off the driving thread.

**Asynchronous (`lgwks_bot`):** write it as a `script!` flow, whose blocks bound
fan-out, deadlines and retries (`skills/lgwks-use-the-crates/SKILL.md`). Below it,
`lgwks_bot::rt` has timers, channels, sockets and `rt::task::join_all_bounded`.
`tokio` is an optional feature of the `lgwks_deps` storefront, the bot reaches it
through this single facade, and the gate refuses any other `tokio` consumer. There
is no attribute macro, because it would expand to `::tokio` paths you cannot resolve
without a `tokio` edge: enter through `lgwks_bot::block_on`.

Boundary: not a realtime scheduler, and worker completion order is
nondeterministic. `net` does not apply the HTTP egress policy. A high-connection
async server is inside this tier now, through the `edge` feature, so do not add
`tokio` yourself.

## syn: do not add a second parser

- Multi-language structure → `lgwks_ast` (`Language::of_path`, `try_parse`,
  `inspect_ast`); grammars are cargo features, and ast-grep types are
  re-exported so consumers never depend on ast-grep directly.
- Rust source scanning → `lgwks_ast::scan` (ADR 0002).
- Rust semantic analysis (types, traits, macro expansion) is not provided;
  state that requirement explicitly rather than introducing a second parser.

## Real gaps (no equivalent, do not fake it)

`anyhow` (use a typed error); `log`/`env_logger` (`tracing` is not a gap:
`lgwks_std::trace` is default-on, and printing from library code is `forbid`);
`clap` (parse `std::env::args`); `toml`/`yaml`/`csv` (use `json`/`ron`); `rand`
distributions (only OS entropy exists).

For any gap: either write the minimal `lgwks_std` module (ELIMINATE), or
register the crate through `skills/lgwks-dependency-admission/SKILL.md` with a
reason naming what `std` cannot do. Never add the edge without the register
entry, because `lgwks-deps check` is the first CI lane and will refuse it.
