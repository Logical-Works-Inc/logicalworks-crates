# Logical Works Codebook

> Provenance: copied verbatim from the estate governance suite
> (`logical-DB/CODEBOOK.md`, `logical-DB/WORKFLOW.md`) on 2026-09-21 and
> maintained here as this repository's copy. Where a rule names this
> repository's own gates, `scripts/ci-local.sh` is the definition.

The code rules for this repository, and its module map. Process is in
`WORKFLOW.md`. Authority, decisions, and the completion ledger are in
`GOVERNANCE.md`. The agent entry point is `AGENTS.md`.

This file and its parts bind every line of source in the repository, including
test code. There is no separate, weaker standard for tests.

## The rules

Each section lives in one part under `docs/codebook/`; a citation of
`CODEBOOK.md §N` resolves through this table.

| § | Rule | Part |
|---|---|---|
| 1 | Precedence | [`lints.md`](docs/codebook/lints.md) |
| 2 | The lint contract: `forbid`, silent traps, coverage ceiling, API bans | [`lints.md`](docs/codebook/lints.md) |
| 3 | Anti-slop invariants: types, errors, arithmetic, patterns, locks | [`anti-slop.md`](docs/codebook/anti-slop.md) |
| 4 | API shape | [`api.md`](docs/codebook/api.md) |
| 5 | Concurrency and resources | [`api.md`](docs/codebook/api.md) |
| 6 | Observability | [`api.md`](docs/codebook/api.md) |
| 7 | Tests | [`tests.md`](docs/codebook/tests.md) |
| 8 | Comments and naming | [`tests.md`](docs/codebook/tests.md) |
| 9 | Suppressions | [`tests.md`](docs/codebook/tests.md) |
| 10 | Dependencies and the reuse ladder | [`dependencies.md`](docs/codebook/dependencies.md) |
| 11 | Banned constructs and their replacement | [`dependencies.md`](docs/codebook/dependencies.md) |
| 12 | What a green gate does not prove | [`dependencies.md`](docs/codebook/dependencies.md) |

## Module map

The crates, what each owns, and the invariants that bind it. The workspace
graph is §10; dependency ownership is `AGENTS.md` and `contract/APPROVED.toml`.

| Crate | Path | Owns | Invariants |
|---|---|---|---|
| `lgwks_std` | `crates/lgwks-std` | the core surface: codecs, time, ids, hashing, regex, JSON/RON/wire, HTTP, trace install; no async runtime | INV-RANDOM-*, INV-ID-*, INV-DEP-20 |
| `lgwks_bot` | `crates/lgwks-bot` | orchestration on one ECS world: `rt`, `script!` flows, journal, supervision | the `lgwks_bot` rows of `INVARIANTS.md` |
| `lgwks_ast` | `crates/lgwks-ast` | the one parser: language identification, tree-sitter grammars, `script!` reading, `scan` | SL-2 |
| `lgwks_macros` | `crates/lgwks-macros` | the `script!` compiler | SL-2 |
| `lgwks_deps` | `crates/lgwks-deps` | the storefront: dependency admission, the gate binaries, the `catalog` command, `ci-local`, the `lgwks` CLI | INV-DEP-EDGE-OWNED |
| `drishti` | `crates/drishti` | frontend orchestration: design publication and a deterministic reference core | the `drishti` rows of `INVARIANTS.md` |

### `lgwks_std::id`

The estate identity type, replacing the `uuid` crate for stored identities
(ADR 0010).

| Module | Owns | Invariant |
|---|---|---|
| `id` | `Uuid`: parse, display, bytes, versions 4 and 7 | INV-ID-1 |
| `id::v7` | the process-wide RFC 9562 method 1 generator; `new_v7` refuses, `now_v7` clamps | INV-ID-2, INV-ID-4 |
| `id::tuple` | the FoundationDB tuple layer: all 12 typecodes (`Element`, `pack_element`, `pack_elements`, `unpack_element`, `unpack_elements`), `0x30` and 16 network-order bytes; `TupleError` | INV-ID-5 |
| `math` | deterministic pure-Rust IEEE 754 `f64` and `f32` elementary functions (`sqrt`, `cbrt`, `exp`, `ln`, `pow`, `sin`, `cos`, `atan2`) | INV-STD-LOCALE-1 |

`Uuid` implements no serde trait (ADR 0010 decision 3).
### `lgwks` (the `lgwks_deps` binary)

| Module | Command |
|---|---|
| `new`, `check`, `expand`, `run` | one-file apps and the Rust they compile to |
| `legacy` | legacy documents read through the estate parsers |
| `id` | `lgwks id new\|key\|read`: version 7 ids and their FoundationDB key bytes (INV-ID-5) |
| `tls` | `lgwks tls`: installs rustls on ring, the agent clients' one TLS provider (`tls-ring`, ADR 0015) |
| `common` | `Refusal`, its kinds, and the shared human and `--json` reporting |

### `check-docs` (a `lgwks_deps` gate binary)

| Module | Owns | Invariant |
|---|---|---|
| `check_docs` | ADR fields (ISO/IEC/IEEE 42010) and PRD attributes (29148); `--base REV`, read through `lgwks_ast::hunks` | INV-DOC-3 |
| `check_docs::prose` | the reader rules: title, opening sentence, fence language, heading levels, live links, 45-word sentences, 8-sentence paragraphs | INV-DOC-5 |

### `lgwks_ast::scan` and `lgwks_ast::hunks`

| Module | Owns | Invariant |
|---|---|---|
| `scan` | `Rule`, `Rule::ALL` (the whole scan) and `Rule::OPT_IN`, `scan_source_with`; `lgwks-ast scan --base REV` keeps findings on added lines | INV-SCAN-ZERO |
| `scan::example` | the opt-in `missing-example` rule: a `pub` library item whose doc opens no fenced block | INV-DOC-6 |
| `hunks` | `added_since`: the lines a change adds per file, the one `git diff --unified=0` reader | INV-DOC-6, INV-DOC-5 |

### `lgwks_ast::script` and `lgwks_ast::diagnostic`

| Module | Owns | Invariant |
|---|---|---|
| `script::lexicon::words` | the core words' rows, `flow` to `fail`; `vocab` holds the structured words | SL-1 |
| `script::tree::blocks` | `Flow`, `Block` and the block words `each`, `within`, `retry`, `step`, `for` | SL-2 |
| `script::tree::branch` | `Together`, `Branch` and the `if` chain | SL-2 |
| `script::tree::feeds` | `Whenever`, `Overflow` and `Justify` | SL-2 |
| `script::tree::map` | `MapStep` and `write_map`, the one map rendering (#384) | SL-2 |
| `script::tree::shape` | `FlowShape`, `StepShape` and `Passthrough`, the map's entries | SL-2 |
| `script::emit::admit` | the domain and outside admissions a flow runs first, and the map entries it emits | SL-2 |
| `diagnostic::tests`, `diagnostic::line_index_tests` | the located-diagnostic and line-index tests | INV-AST-2 |
| `error_derive` | zero-`syn` `#[derive(Error)]` token-stream state machine and `define_error!` macro, replacing `thiserror` (#842) | INV-DEP-20 |

### `lgwks_deps` capability modules

| Module | Owns | Invariant |
|---|---|---|
| `advisories` | offline RustSec advisory parser and semver evaluator; `lgwks-deps advisories` (#799) | INV-SEC-AUDIT-1 |
| `sqlite` | the one opener and `migrate`: serialized first open, one transaction per step (#872, #873) | INV-DEP-EDGE-OWNED |

### `lgwks_bot::rt::serve`, `lgwks_bot::task` and `lgwks_bot::journal`

| Module | Owns | Invariant |
|---|---|---|
| `kernel`, `clock::epoch` | the supported paths; `EpochClock`, `RecordedInstant`, `recorded-instant` (ADR 0021) | INV-KERNEL-1, INV-EPOCH-1 |
| `journal::records`, `journal::file::corruption` | `RecordJournal`, the durable log over a consumer's own record (ADR 0016); the typed account of a refused frame | INV-RECORD-1 |
| `broker::fence` | authenticate, withdraw by name, the offline count, adoption over records, `broker-fence` (ADR 0017) | INV-FENCE-1 |
| `ecs::projection` | `RecordProjection`, a consumer's fold on the ECS world, `record-projection` (ADR 0018) | INV-PROJECTION-1 |
| `script::journaled_cancel` | a scope's stop in the consumer's journal, `durable-stop` (ADR 0019) | INV-STOP-1 |
| `ecs::effects::outcomes`, `journal::seam` | settling what an attempt became; the `EffectJournal` trait | INV-BOT-14 |
| `app::auth` | `Principal`, `RequestHeaders`: who a request is, at the edge (ADR 0020) | INV-EDGE-1 |
| `script::run_store::step`, `task::disposition` | the durable step; task disposition and fault attribution | INV-BOT-14 |
| `rt::serve::ingress`, `task::store::append` | request ingress and retry answering; ordered append on the storage owner thread | INV-BOT-50 |
