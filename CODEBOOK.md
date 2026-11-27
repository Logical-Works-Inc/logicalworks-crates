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
| `lgwks_std` | `crates/lgwks-std` | the core surface: codecs, time, ids, hashing, regex, `glob` (strict and legacy compilers; a strict refusal names its UTF-8 byte offset, held by `sim_glob_refusal`, ADR 0068), JSON/RON/wire, HTTP (`http::response`, `Response` and the `Head` a streamed response shares; `http::stream::sse_decode`, the I/O-free event decoder; `http::tls::der`, the X.509 and key walk), process control (`process::wait`, the pure reading of a Windows handle wait that decides liveness, #913; `capture/table.rs` holds the `/proc`, `sysctl`, `ps` and Toolhelp readings; `process::orphans`, the census a subreaper takes of what its child left, ADR 0082; `process::lifeline`, the `sh` that kills held groups, detaches held disks and runs held releases when its owner dies, ADR 0087, and `lifeline::ledger`, the flock'd lease records the first hold of each process sweeps, ADR 0090), trace install, `fs::stream` (the streaming walk, every link listed; `sim_links` checks it against `read_dir`), `fs::capability` (handle-relative `Dir`; `capability/raw.rs` holds its private syscall helpers), `task` (the bounded blocking pool; `task/state.rs` holds the state its lock guards and every transition on it, `task/sync.rs` the poison-tolerant lock helpers); no async runtime | INV-RANDOM-*, INV-ID-*, INV-DEP-20, INV-STD-HTTP-3, INV-TLS-2, INV-STD-PROCESS-2, INV-CI-ORPHANS-1, INV-STD-LIFELINE-1, INV-STD-LEDGER-1, INV-FS-8 |
| `lgwks_bot` | `crates/lgwks-bot` | orchestration on one ECS world: `rt`, `script!` flows, journal, supervision, and `interface` element resolution, whose scoring rules and eligibility gates are `interface/scoring.rs`; the `edge-load` bin's `edge_load/drive.rs` (the seeded load client), `edge_load/rig.rs` (`--rig`: every edge surface on one workload, one SQLite row per round, #1007) and `edge_load/usage.rs` (a server's CPU and peak RSS); the `estate` bin's `vocabulary/clock_wait.rs` (`Clock::sleep` on both sources, on the shipped binary); `rt/serve/runs.rs` owns every admitted serve run's task in a `JoinSet` the drain joins or aborts, so a hung-up client never drops a run (ADR 0056, INV-SERVE-1); `rt/serve/hosts.rs` is the tenant host table, 64 shards with a built-once slot per tenant and a 1,024-tenant cap (ADR 0057, INV-SERVE-2); `rt/serve/table.rs` builds each route's task once at start (ADR 0059, INV-SERVE-3). one typed error per layer, each raised where it is defined and wrapped upward by `From` (ADR 0061, INV-ERROR-LAYER-1): `error` (`KernelError`, `RetryClass`), `ecs/error` (`EcsError`), `session/error` (`SessionError`), `script/error` (`FlowError`, rendered by `script/error/display.rs`) and `app_error` (`BotError`, the app layer's sum); `domain/cas.rs` is the `CasWrite` verb adapter over the kernel's `cas` store. `world`, the owned per-tenant store (typed columns, change ticks, transactions, snapshots, one world hash; ADR 0055), private until #1004, with its sims in `sim_world/` and the `world-bench` bin. Planned (ADR 0052): renamed `lgwks_runtime`, a facade over `lgwks_kernel`, `lgwks_rt`, `lgwks_world`, `lgwks_session`, `lgwks_script`, `lgwks_domains` and `lgwks_app`, each planned until its carve lands | `crates/lgwks-bot/INVARIANTS.md` and its numbered parts, INV-WORLD-1 |
| `lgwks_ast` | `crates/lgwks-ast` | the one parser: language identification, tree-sitter grammars, `script!` reading, `scan` | SL-2 |
| `lgwks_macros` | `crates/lgwks-macros` | the `script!` compiler; `lexicon_docs` renders the README table, the `docs/script-lexicon*.md` pages and each crate's `src/lexicon.md` from `LEXICON`, and `lexicon_docs/page_error.rs` is its typed `PageError` | SL-2 |
| `lgwks_deps` | `crates/lgwks-deps` | the storefront: dependency admission, the gate binaries, the `catalog` command, `ci-local` (whose `prove` lane is the meta prover: each kind of `contract/RESOURCES.toml` holds a closure lane, a Kani model and a census, ADR 0091), the `lgwks` CLI, and `churn-slot`, which shares `src/bin/shared/ram_disk.rs` with `ci-local` and keeps its unit tests in `src/bin/churn_slot/slot_tests.rs` (ADR 0026), and whose `churn_slot/shared_volume.rs` holds the shared volume per wrapper and detaches it once idle (ADR 0087), and whose `churn_slot::residue` is the census every test runs under: a fresh `TMPDIR` and its leftovers named, swept by `churn_slot/sim_residue.rs` (ADR 0078); `src/bin/shared/rust_code.rs` is the one lexer `check-std-first` and the `ci_local::test_gated` unwrap lane share (ADR 0035); `ci_local::invariants::registers` finds each register part and holds it to 120 lines at 120 columns (ADR 0040); `ci_local::runners` plans the docs.rs `cargo doc` runs on the pinned nightly, held by `ci_local/sim_runners.rs` (ADR 0066); `ci_local::skill_hygiene` is the `skill-hygiene` lane, every skill's frontmatter, route, fences and doctest include and every harness link into `skills/`, held by `tests/it/sim_skill_hygiene.rs` (ADR 0084); `check_std_first::scans::guessed_path` refuses a binary path guessed by navigating the running binary's (ADR 0066); `invariants::parse` builds one `[[invariant]]` entry from the line reader's raw block and refuses what the schema cannot represent; `metadata::sim_capture_faults` attacks the caller-passed fault plan and the capture owner against a reference queue of armed points; `ci-receipt` turns a failed run's job logs into one receipt: `ci_receipt::logs` lists and tail-reads them, `line` cleans a line, `steps`, `nextest` and `compile` read the failing step, test and diagnostic, `failure::Kept` holds each reader to one key once and 2,048 a log, `failure::Receipt` keeps the sorted first `--max`, `render` writes the JSON and the escaped table, with sims in `sim_ci_receipt.rs` (reading) and `sim_ci_receipt_out.rs` (the receipt) (ADR 0086) | INV-DEP-EDGE-OWNED, INV-CI-SLOT-1, INV-SCAN-GATED-1, INV-DOC-8, INV-CI-BRITTLE-1, INV-CI-RESIDUE-1, INV-CI-FAIL-RECEIPT-1, INV-DOC-10 |
| `drishti` | `crates/drishti` | the frontend framework and the one home of Canvas (ADR 0058): `canvas_protocol`, `canvas_scene`, `motion`, `preview`, and `native_host`, whose tenant-keyed broker, audit sink, governance store and attestation evidence take `lgwks_deps` features `p256`, `libm` and `desktop-preview` | the `drishti` rows of `INVARIANTS.md` |

### `lgwks_std::id`, `math` and `legacy::fax`

The estate identity type, replacing the `uuid` crate for stored identities (ADR 0010). `Uuid` implements no serde trait (ADR 0010 decision 3).

| Module | Owns | Invariant |
|---|---|---|
| `id` | `Uuid`: parse, display, bytes, versions 4 and 7 | INV-ID-1 |
| `id::v7`, `id::loom_v7_tests` | the process-wide RFC 9562 method 1 generator; `new_v7` refuses, `now_v7` clamps; loom proves the lock order is the issue order (ADR 0030) | INV-ID-2, INV-ID-4 |
| `id::tuple` | the FoundationDB tuple layer: all 12 typecodes (`Element`, `pack_element`, `pack_elements`, `unpack_element`, `unpack_elements`), `0x30` and 16 network-order bytes; `TupleError`; `id::sim_tuple_ints` holds every integer width against its byte order | INV-ID-5 |
| `math` | deterministic pure-Rust IEEE 754 `f64` and `f32` elementary functions (`sqrt`, `cbrt`, `exp`, `ln`, `pow`, `sin`, `cos`, `atan2`) | INV-STD-LOCALE-1 |
| `legacy::fax`, `fax::bitmap`, `fax::t4`, `fax::t6`, `fax::tiff` | CCITT G3/G4 TIFF decode and encode: `decode_tiff`, `encode_tiff`, `TiffImage`; `bitmap` owns pixel masks, span fills, a strip's row windows and the `Corrupt` marker a row names as `CorruptStrip` | tests `sim_legacy_fax`, `sim_legacy_edges` |

### `lgwks` (the `lgwks_deps` binary)

| Module | Command |
|---|---|
| `new`, `check`, `expand`, `run` | one-file apps and the Rust they compile to |
| `legacy` | legacy documents read through the estate parsers |
| `id` | `lgwks id new\|key\|read`: version 7 ids and their FoundationDB key bytes (INV-ID-5) |
| `tls` | `lgwks tls`: installs rustls on ring, the agent clients' one TLS provider (`tls-ring`, ADR 0015) |
| `common` | `Refusal`, its kinds, and the shared human and `--json` reporting |

### `check-docs` and `check-gate-parity` (`lgwks_deps` gate binaries)

| Module | Owns | Invariant |
|---|---|---|
| `check_docs` | ADR fields (ISO/IEC/IEEE 42010) and PRD attributes (29148); `--base REV`, read through `lgwks_ast::hunks` | INV-DOC-3 |
| `check_docs::prose` | the reader rules: title, opening sentence, fence language, heading levels, live links, 45-word sentences, 8-sentence paragraphs, and the voice rule (no em dash joining statements, no sales phrase) | INV-DOC-5 |
| `check_gate_parity::lane_split`, `sim_lane_split`, `neuter`, `workflow_calls`, `cache_budget`, `platform_gates`, `partitions`, `sim_partitions`, `doc_path`; `linux_container_tests::lane_tests` | rule 6: a test moved to `app-builds` runs once, never twice or never; the Linux leg's suites are the lanes' commands (ADR 0028); rule 7, `neuter`: no gate step swallows its failure and no job or step is `if: false` or `continue-on-error: true` (ADR 0035); rule 8: no workflow calls another repository's (ADR 0029); rule 9: a sliced row's step passes `--partition` (#923); `cache_budget`: only the builds that set the wall save, and `cache-budget.yml` prunes by last use (ADR 0041); `platform_gates`: macOS runs only the platform gates, at most 25 jobs a run (ADR 0042); rule 10, `wrapper`: every toolchain the hosted action installs carries the `clippy-driver` the cargo config runs; rule 11, `doc_path`: only the required row waits on the doc tests, whose cache key is their own (ADR 0076); rule 12, `bounded_fetch`: every `curl` of every workflow and action has a `--max-time` (ADR 0088) | INV-CI-RUNNER-CLASS-1, INV-CI-NEUTER-1, INV-CI-CACHE-1, INV-CI-PLATFORM-1, INV-CI-TOOLCHAIN-2, INV-CI-DOC-PATH-1, INV-CI-FETCH-1 |

### `lgwks_ast::scan` and `lgwks_ast::hunks`

| Module | Owns | Invariant |
|---|---|---|
| `scan` | `Rule`, `Rule::ALL` (the whole scan) and `Rule::OPT_IN`, `scan_source_with`; `lgwks-ast scan --base REV` keeps findings on added lines; `scan::raw_process`, the opt-in `raw-process` rule (a `std` or `tokio` process `Command::new` by path, import or alias, tests and macro arguments included), which `lgwks-deps gate` runs as a leg | INV-SCAN-ZERO, INV-SCAN-RAW-PROCESS-1 |
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
| `diagnostic::tests`, `diagnostic::line_index_tests`; `it::hostile::markdown` | the located-diagnostic and line-index tests; the markdown guard proved from child processes | INV-AST-2 |
| `error_derive` | zero-`syn` `#[derive(Error)]` token-stream state machine and `define_error!` macro, replacing `thiserror` (#842) | INV-DEP-20 |

### `lgwks_deps` capability modules

| Module | Owns | Invariant |
|---|---|---|
| `advisories` | offline RustSec audit: `audit` entry, `semver` (Cargo's reader), `db` layout and matching; the binary's `bin/deps_cmd/advisories.rs` renders it; `git` is the crate's one git launcher; a match's title is the first heading (#799, #942, #955) | INV-SEC-AUDIT-1, INV-SEC-AUDIT-2, INV-SEC-AUDIT-3 |
| `sqlite` | the one opener and `migrate`: serialized first open, one transaction per step (#872, #873) | INV-DEP-EDGE-OWNED |

### `lgwks_bot::rt::serve`, `lgwks_bot::task` and `lgwks_bot::journal`

| Module | Owns | Invariant |
|---|---|---|
| `kernel`, `clock::epoch` | the supported paths; `EpochClock`, `RecordedInstant`, `recorded-instant` (ADR 0021) | INV-KERNEL-1, INV-EPOCH-1 |
| `journal::records`, `journal::file::corruption`, `durable`, `journal::continuation::{naming, watermark}` | `RecordJournal`, the durable log over a consumer's own record (ADR 0016); the typed account of a refused frame; the one synced whole-document replace (#910); where a successor journal lives and when a journal is due to continue, crate-private, swept by `continuation/sim_naming.rs` (ADR 0064) | INV-RECORD-1, INV-BOT-193 |
| `broker::fence`, `ecs::projection` | authenticate, withdraw by name, the offline count, adoption over records, `broker-fence` (ADR 0017); `RecordProjection`, a consumer's fold on the ECS world, `record-projection` (ADR 0018) | INV-FENCE-1, INV-PROJECTION-1 |
| `frontier::permit`, `domain::gh`, `ecs::builder::debug`, `journal::file::sealed_impl::codec` | the `InFlightPermit` a frontier admission reserves; `Gh::snapshot`'s twin for a build without `process`, refusing as `NoRunner` (`tests/it/t_rows_default/review.rs`), the review reads living beside `review` only; `Debug` for the bot and its builders, written by hand across their `dyn` seams; the read side of the sealed frame codec, classifying each committed payload at open | INV-BOT-14 |
| `script::journaled_cancel` | a scope's stop in the consumer's journal, `durable-stop` (ADR 0019) | INV-STOP-1 |
| `ecs::effects::outcomes`, `journal::seam`, `journal::file::sim_adapter`, `effect::id` | settling what an attempt became; the `EffectJournal` trait, and the file adapter's seeded receipt sim for `confirm_outcome` in its own module path; the never-zero role ids an `EffectKey` names | INV-BOT-14, INV-RANDOM-1 |
| `app::auth`, `app::command`, `app::budget` | `Principal`, `RequestHeaders`: who a request is, at the edge (ADR 0020); `CommandLimits` and the pool a held command runs on, `command-journal` (ADR 0025); `BudgetLimits`, each tenant's effect allowance per window (ADR 0054) | INV-EDGE-1, INV-COMMAND-1, INV-BUDGET-1 |
| `script::run_store::{step, conformance}`, `script::ask`, `script::scope::run_state`, `task::disposition`, `task::host::approvals`, `task::ledger::disk`, `task::store::record::replay` | the durable step, which `ask` records its whole model answer through, replayed on resume and `Drift::Input` under another prompt (ADR 0070, INV-BOT-198), its token budget reserved from a `justify` token rule inside the step (ADR 0071, INV-BOT-199); the kit every `RunRecords` runs, whose `first_attempt_is_compatible` holds a run with no records compatible (`tests/it/sim_first_attempt.rs`); the run state a scope carries; task disposition and fault attribution; the host's approval door, the ledger's disk half and record replay, each split from its owner by responsibility | INV-BOT-14, INV-BOT-195 |
| `rt::serve::ingress`, `rt::apps::held`, `task::store::append`, `journal::owner::store::batch`, `rt::sweep`, `task::store::sealed_read`, `rt::supervise::scratch` | each supervised child's private `TMPDIR`, removed with its group guard, and the sweep of dead roots and idle `xcrun_db-*` files, with its judges' sims in `supervise/sim_scratch.rs` (ADR 0077, INV-BOT-TEMP-1); request ingress and retry answering; the plans each tenant holds, with a floor of 4 no neighbour can take (#889); ordered append on the storage owner thread; that thread's group-commit loop, on its own thread and asleep until signalled (#1041); the TTL `Sweeper` that runs the kernel's `sealed::sweep::SweepTarget`; the run store's `SealedStepRead` and `ShredOutcome` (#991); `sealed::conformance`, the kit an outside `KeyProvider` runs against itself, run by `store-conformance` (#1060) | INV-BOT-50, INV-APPS-STORE-1 |
