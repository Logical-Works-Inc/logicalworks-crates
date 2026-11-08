# ADR 0014: capability gaps split between first-party elimination and storefront boundaries

Status: **proposed**, 2026-11-05. Covers #794, #798, #799, #808, #814, #833, #836, #837, #838, #840, #841, #842, #846, #848, #849, #850, #851, and #854.

Stakeholders: the Director; downstream consumers (`logical_ci`, `logical-DB`, `nova`, `secure-authority`, `rocco`, and canvas-absorption crates).
Concerns: supply-chain bloat from habits (`thiserror`, `libm`, `rustsec`, `foundationdb-tuple`), `libsqlite3-sys` linkage collisions, polling start-gate overhead, and `unsafe` leakage across workspace crates.
Requirements: R1, R3, R4, R10, R11, R12, R13
Viewpoint: the dependency admission viewpoint (`docs/dependency-doctrine.md`), mapping each capability gap to either Rung 5 (`ELIMINATE`) or Rung 4/8 (`BOUNDARY` storefront).
Elements and relations: `lgwks_std::{id::tuple, process, math, ec}`, `lgwks_bot::{ecs, rt::process, rt::supervise}`, `lgwks_macros::Error`, `lgwks_ast::error_derive`, and `lgwks_deps::{advisories, sqlite, vz, warden, storefront}`.

## Context

Eighteen open capability issues span `lgwks_std`, `lgwks_bot`, `lgwks_macros`, and `lgwks_deps`. Six describe fixed algorithms or kernel primitives (`#798` process snapshots, `#814`/`#808` pipe gates, `#851` FDB tuples, `#842` `Error` derive, `#837` float math, and `#799` RustSec evaluation). Seven cover cryptography, databases, and web routing (`#841` `appcui_proc_macro`, `#849`/`#848` SQLite and `loco-rs`, and `#794`/`#840`/`#833` NIST curves, X.509, CMS, and PDF signing). The remaining five cover sandboxes, virtual machines, remote MCP, preview UI, and property testing (`#846` warden, `#854` Apple VZ, `#850` `rmcp`, `#836` preview UI, and `#838` `proptest`).

## Decision

1. **Eliminate six capabilities in-tree (Rung 5).** Implement `capture_descendants_many` (`#798`), `Element` tuple packing (`0x00`..`0x33`, `#851`), and deterministic IEEE 754 `math` (`#837`) in `lgwks_std` with zero new external dependencies. Implement inherited-FD passing (`#814`), anonymous-pipe start gates (`#808`), and `EntityUuid`/`TenantEntityIndex` (`#851`) in `lgwks_bot`. Implement `#[derive(Error)]` (`#842`) in `lgwks_macros` and `lgwks_ast`, retiring `thiserror` from `contract/APPROVED.toml`. Implement local RustSec lockfile evaluation (`#799`) in `lgwks_deps::advisories` over existing lockfile, SemVer, and BLAKE3 primitives.
2. **Isolate NIST curves in `lgwks_std::ec` (`#794`, `#840`).** Wrap pure-Rust `p256`, `p384`, and `p521` (`=0.14.0`, `arithmetic`, `ecdsa`, `zeroize`) behind `lgwks_std::ec` with strict low-`S` verification, SEC1 point encoding, and scalar multiplication.
3. **Unify SQLite linkage and admit storefront boundaries (Rung 4/8).** Align `rusqlite` and `sqlx-sqlite` on one `libsqlite3-sys` version (`#849`) and extract `SqliteExecutor` so both engines share the `WAL` + `synchronous = FULL` + `user_version` contract. Pin `appcui_proc_macro = "=0.3.6"` (`#841`). Admit `web-loco` (`#848`), `pki-x509` and `pdf-sign` (`#833`), `warden-wasm`/`warden-seccomp`/`warden-wat` (`#846`, retiring `wasmtime` from `contract/DEFERRAL.toml`), `apple-vz` (`#854` safe `lgwks_deps::vz` wrapper), `mcp-remote` (`#850`), `ui-preview` (`#836`), and `proptest` (`#838`) as default-off `lgwks_deps` features with schema-3 `[approved.retire]` mortgages.

## Alternatives rejected

- **Admitting `foundationdb-tuple`, `thiserror`, `libm`, and `rustsec` as storefront dependencies.** Each duplicates a parser, digest, or fixed wire format the workspace already owns, increasing supply-chain surface and cold compile times.
- **Keeping `SIGSTOP` polling for gated process spawns (`#808`).** Costs a 5 ms poll loop per lane and cannot fail closed before `exec` when the coordinator is killed with `SIGKILL`.
- **Exposing raw `objc2-virtualization` types to downstream crates (`#854`).** Forces `unsafe` Objective-C message sends into consumer crates that enforce `#[forbid(unsafe_code)]`.

## Consequences

- `thiserror` is retired from `contract/APPROVED.toml` and `contract/LIFECYCLE.toml`, saving 3.70 s of cold build time across 6 crates.
- `wasmtime` moves from `contract/DEFERRAL.toml` into `contract/APPROVED.toml` under `warden-wasm` because `nova#93` satisfies its fuel, epoch, and memory bound trigger.
- Every default-off storefront addition leaves `lgwks_std` (`core` + `trace`) and `lgwks_bot` default feature closures unchanged and `wasm32-wasip1` compatible.

## Correspondence

Refines [ADR 0003](0003-every-capability-on-by-default.md) and [ADR 0010](0010-estate-uuid-replaces-the-uuid-crate.md). Normative specifications live in [docs/std-deps-capability-gaps.spec.md](../std-deps-capability-gaps.spec.md) and [docs/std-deps-storefront-gaps.spec.md](../std-deps-storefront-gaps.spec.md).

## Verification

- `cargo run -p lgwks_deps -- check .` and `cargo run -p lgwks_deps --bin catalog -- --check` enforce ownership, default-feature rules, and schema-3 mortgages.
- `crates/lgwks-deps/tests/storefront_consumers.rs` verifies isolated and combined compilation (including `--features sqlite,web-loco` and fresh `appcui` resolution).
