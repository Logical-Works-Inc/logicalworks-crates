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
| `lgwks_deps` | `crates/lgwks-deps` | the storefront: dependency admission, the gate binaries, `ci-local`, the `lgwks` CLI | INV-DEP-EDGE-OWNED |
| `drishti` | `crates/drishti` | frontend orchestration: design publication and a deterministic reference core | the `drishti` rows of `INVARIANTS.md` |

### `lgwks_std::id`

The estate identity type, replacing the `uuid` crate for stored identities
(ADR 0010).

| Module | Owns | Invariant |
|---|---|---|
| `id` | `Uuid`: parse, display, bytes, versions 4 and 7 | INV-ID-1 |
| `id::v7` | the process-wide RFC 9562 method 1 generator; `new_v7` refuses, `now_v7` clamps | INV-ID-2, INV-ID-4 |
| `id::serde_impl` | text when human-readable, 16 bytes otherwise | INV-ID-3 |
| `id::tuple` | the FoundationDB tuple element: `0x30` and 16 network-order bytes; `TupleError` | INV-ID-5 |

Open on #851: the rest of the tuple layer, the ECS identity surface, CBOR.

### `lgwks` (the `lgwks_deps` binary)

| Module | Command |
|---|---|
| `new`, `check`, `expand`, `run` | one-file apps and the Rust they compile to |
| `legacy` | legacy documents read through the estate parsers |
| `id` | `lgwks id new\|key\|read`: version 7 ids and their FoundationDB key bytes (INV-ID-5) |
| `common` | `Refusal`, its kinds, and the shared human and `--json` reporting |
