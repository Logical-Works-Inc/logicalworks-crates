# lgwks_std consumer guide

Everyday Rust primitives with no async runtime required, each behind one
audited dependency stack. The package is `lgwks_std` (underscore) in the
directory `crates/lgwks-std` (hyphen), used as `use lgwks_std::...`.

The [root README](../../../README.md#crates) lists this tree's version. MSRV: Rust
1.98.0, edition 2024 (`crates/lgwks-std/Cargo.toml`).

## Install

```sh
cargo add lgwks_std                                     # standard: all but http
cargo add lgwks_std --features http                     # add the HTTP client
cargo add lgwks_std --no-default-features --features core   # zero external deps
```
## Feature selection

The default is `standard`, every capability but `http`, whose TLS stack compiles C
per target (ADR 0003). `full` adds it.

| Feature | Modules | External deps |
|---|---|---|
| `core` | `encoding`, `fs`, `glob`, `hex`, `leb128`, `retry`, `task`, `time` | none |
| `trace` | `trace` | `tracing` |
| `trace-install` | the default subscriber bootstrap | `tracing-subscriber` stack |
| `random` | `random`, `id` | `getrandom` |
| `hash` | `hash` | `blake3` |
| `build` | a build script's input digest and directives | none beyond `hash` and `trace` |
| `sign` | `sign` | `ed25519-dalek` |
| `pattern` | `pattern` | `regex` |
| `json` | `json` | `serde`, `serde_json` |
| `ron` | `ron` | `serde`, `ron` |
| `wire` | `wire` | `rkyv` |
| `legacy` | `legacy` | none |
| `http` (not in `standard`) | `http` | `ureq`, `iri-string` |
| `online` | `online` | none |
| `fs-raw` | `fs` | `rustix` |
| `process` | `process` | `rustix` on Unix; none on Windows |
| `standard` (default) | all of the above but `http` | all of those |
| `full` | all of the above | all of the above |

Features compose, and `--no-default-features` lets you pick only the ones you use.

### The zero-dependency configuration

There is exactly one configuration of this crate that pulls nothing at all, and
it needs both halves:

```toml
[dependencies]
lgwks_std = { version = "4", default-features = false, features = ["core"] }
```
`default-features = false` removes `standard`. `features = ["core"]` puts the core
modules back. A bare `lgwks_std = "4"` is **not** zero-dependency: it is `standard`.

This is checkable rather than assertable. From a clean project:

```sh
cargo tree -e normal | grep lgwks_std
```
Against this repository, `cargo tree -p lgwks_std --no-default-features
--features core -e normal` prints a single line, the crate itself, and nothing
beneath it. The default build prints every stack of `standard` under it.

## More pages

- [HTTP](http.md): the pooled client, streamed bodies and server-sent events, pagination and the mTLS seam.
- [Logging](logging.md): why library code never prints, and `trace`.
- [Notes and glob matching](details.md): support-round notes and the glob dialects.
- [Reference](reference.md): the feature table, module reference, entropy targets and walk threat models.
- [Time and glob notes](time-and-glob.md): the RFC 3339 profile and the glob text semantics.

## Modules

| Module | What it does |
|---|---|
| `encoding` | base64 and percent-encoding |
| `fs` | recursive directory walking with sandbox enforcement |
| `glob` | shell-style glob matching |
| `hex` | hex encode and decode |
| `leb128` | LEB128 variable-length integers |
| `retry` | retry budgets: attempts, exponential backoff with caller jitter, deadlines |
| `task` | single-threaded executor: `block_on`, concurrent `join_all`, off-thread `spawn_blocking` |
| `time` | RFC 3339 timestamps, calendar math |
| `random`, `id` | OS entropy, UUID v4 and v7, the FoundationDB tuple UUID element |
| `hash` | BLAKE3 |
| `pattern` | compiled regex |
| `json`, `ron`, `wire` | serialization; `json::Value` is this crate's own tree |
| `legacy` | EBCDIC, packed decimal, COBOL copybooks, compound files and VBA, Access rows, fax images |
| `http` | blocking HTTPS client, rustls-only TLS |
| `online` | TCP reachability probing |

## Where to go next

- [lgwks_bot](../lgwks-bot/index.md) for capability-gated automation, which
  depends on this crate with `json` and `http` enabled.
- [lgwks_ast](../lgwks-ast/index.md) for the parser and the diagnostic derive.
- [lgwks_deps](../lgwks-deps/index.md) if you want the admission gate on your own
  workspace.
