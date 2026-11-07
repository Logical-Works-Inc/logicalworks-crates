# System and ecosystem capability gaps: storefront boundaries and verification

Part two of [the system and ecosystem capability gap specification](std-deps-capability-gaps.spec.md), split under the 120-line Markdown rule. This page specifies the `lgwks_std::ec` cryptographic surface, the `lgwks_deps` storefront boundary features (`#841`, `#849`, `#848`, `#794`, `#840`, `#833`, `#846`, `#854`, `#850`, `#836`, and `#838`), the nine-axis compliance matrix, and the rollout waves.

## 1. Storefront and cryptographic boundary table

| Issues | Feature / module | Pinned upstream crates | Retirement replacement (`[approved.retire]`) |
|---|---|---|---|
| `#841` | `lgwks_deps` `appcui` | `appcui =0.5.5`, `appcui_proc_macro =0.3.6` | `drishti::terminal` |
| `#849`, `#848` | `lgwks_deps` `sqlite`, `web-loco` | `rusqlite =0.39.0`, `loco-rs =1.2.0`, `sea-orm =2.0.4`, `axum =0.8.9`, `async-trait =0.1.89` | `lgwks_bot::rt::serve` |
| `#794`, `#840` | `lgwks_std::ec` (`ec`) | `p256 =0.14.0`, `p384 =0.14.0`, `p521 =0.14.0` (`arithmetic`, `ecdsa`, `zeroize`) | `lgwks_std::ec` |
| `#833` | `lgwks_deps` `pki-x509`, `pdf-sign` | `der`, `spki`, `pkcs8`, `x509-cert`, `cms`, `lopdf` | `lgwks_std::sign` |
| `#846` | `lgwks_deps` `warden-wasm`, `warden-seccomp`, `warden-wat` | `wasmtime =36.0.14`, `seccompiler =0.5.0` (Linux), `wat =1.240.0` | `lgwks_bot::warden` |
| `#854` | `lgwks_deps::vz` (`apple-vz`, macOS) | `objc2-virtualization =0.3.2`, `objc2-foundation =0.3.2`, `block2 =0.6.2` | `lgwks_std::process` |
| `#850` | `lgwks_deps` `mcp-remote` | `rmcp =3.8.0` (`server`, `transport-streamable-http-server`, `auth`) | `lgwks_bot::rt::serve` |
| `#836` | `lgwks_deps` `ui-preview` | `winit`, `softbuffer`, `cosmic-text` | `drishti::native` |
| `#838` | `lgwks_deps` `proptest` | `proptest =1.11.0` (`std`, `allowed_kinds = "normal,dev"`) | `lgwks_std::seeded` |

## 2. Storefront and cryptographic specifications

- **AppCUI proc-macro pin (`#841`).** `appcui 0.5.5` declares `appcui_proc_macro ^0.3.6`, which resolves to `0.3.8` on a fresh `Cargo.lock` and emits `GenericLineChartEvents` absent from `0.5.5`. Pin `appcui_proc_macro = "=0.3.6"` under the `appcui` feature and verify fresh unlocked resolution in `storefront_consumers`.
- **SQLite linkage unification and `web-loco` (`#849`, `#848`).** Align `rusqlite` (`=0.39.0`) and `sqlx-sqlite` (`0.9.0` via `loco-rs =1.2.0` / `sea-orm =2.0.4`) on `libsqlite3-sys 0.37` so `--features sqlite,web-loco` resolves a single `links = "sqlite3"` node. Expose `SqliteExecutor` and `PRAGMA_INIT_SQL` (`WAL`, `synchronous = FULL`, `busy_timeout = 60000`, `foreign_keys = ON`) in `lgwks_deps::sqlite` so both `rusqlite::Connection` and `sqlx::SqlitePool` enforce `INV-SQLITE-1`.
- **NIST curves, X.509, CMS, and PDF signing (`#794`, `#840`, `#833`).** The `lgwks_std::ec::{p256, p384, p521}` surface exposes prime-order `Scalar` and `Point` arithmetic (`generator`, `identity`, `add`, `double`, `mul_scalar`, SEC1 compress/decompress) for `logical_ci` key-material checks. It also provides low-`S` canonical ECDSA (`verify_strict`, `verify_prehash`) for Secure Enclave and TrustZone attestations. Storefront features `pki-x509` (`der`, `spki`, `pkcs8`, `x509-cert`, `cms`) and `pdf-sign` (`lopdf`) cover PKI and PAdES, while `base64`, `chrono`, and `reqwest` stay on `lgwks_std::{encoding, time, http}`.
- **Nova warden sandboxes (`#846`).** `nova#93` satisfies `wasmtime`'s trigger in `contract/DEFERRAL.toml`. Remove `wasmtime` from `DEFERRAL.toml` and expose `lgwks_deps::warden` requiring `WasmLimits { max_memory_bytes: NonZeroUsize, max_table_elements: NonZeroU32, fuel_budget: NonZeroU64, epoch_deadline: NonZeroU64 }` at store construction (`R11`), plus `warden-seccomp` on Linux and `warden-wat` for test fixtures.
- **Safe Apple `Virtualization.framework` wrapper (`#854`).** `lgwks_deps::vz` (macOS-only, feature `apple-vz`) encapsulates `objc2-virtualization` inside `MainThreadToken`, `LinuxVmSpec`, and `VirtualMachine` (`build`, `start`, `stop`, `connect_vsock`), converting `NSError` callbacks into typed `VzError` values over `oneshot` channels so downstream crates keep `#[forbid(unsafe_code)]`.
- **Remote MCP (`#850`), UI preview (`#836`), and `proptest` (`#838`).** `mcp-remote` re-exports `rmcp` with Streamable HTTP and OAuth 2.1 only (local stdio MCP stays on `lgwks_std::json` + `lgwks_bot::rt`). `ui-preview` re-exports `winit`, `softbuffer`, and `cosmic-text`. `proptest` re-exports `proptest` with `allowed_kinds = "normal,dev"` for downstream `[dev-dependencies]`.

## 3. Nine-axis verification matrix and rollout order

| Axis | Guarantee across all 15 workstreams | Discriminating falsifier |
|---|---|---|
| **Frontier** | Eliminates `thiserror`, `uuid`, `foundationdb-tuple`, `libm`, and `rustsec`; unifies `libsqlite3-sys`; replaces `SIGSTOP` polling with pipe EOF/write gates. | `sim_tuple_full`, `sim_error_derive`, `sim_math`, `sim_advisories`, and pipe-gate latency bench (`< 50 us`). |
| **Hyperscale** | Bounds `capture_descendants_many` (`16_384` roots, `4_096` PIDs), tuple keys (`32` depth, `10_000` bytes), inherited FDs (`8`), and WASM stores (`WasmLimits`). | `sim_capture_many` (10,000 roots in 1 table read); `unbounded_wasm_store_is_unconstructible`. |
| **Idiomatic** | Typed `#[non_exhaustive]` error enums, zero `.unwrap()`/`.expect()`, `MainThreadToken` thread proof, and `OwnedFd` RAII ownership. | `cargo clippy --workspace --all-targets --locked -- -D warnings`. |
| **Generalized** | Full 12-code FDB tuple grammar (`0x00`..`0x33`) with nested null escaping; generic/lifetime `#[derive(Error)]`; dual `SqliteExecutor`. | `sim_tuple_full` (5,000 schemas) and `sim_error_derive` (1,000 enum/struct shapes). |
| **Decoupled** | Preserves `std -> lgwks_std -> lgwks_bot -> lgwks_ast -> lgwks_deps`; keeps web/ORM in `loco-rs`; mortgages every external edge. | `cargo run -p lgwks_deps -- check .` and `storefront_consumers`. |
| **Ephemeral** | Pipe start gate exits `99` on coordinator `SIGKILL` via kernel write-end `EOF` before `exec`; `SqliteExecutor` enforces `WAL` + `FULL`. | `coordinator_sigkill_before_release_exits_gated_child_without_running_payload`. |
| **Portable** | Bit-identical IEEE 754 `math` and pure-Rust NIST curves on Darwin, Linux, Windows, and `wasm32-wasip1`; target-gated `apple-vz` and `warden-seccomp`. | `cargo run -p lgwks_deps --bin check-target-matrix` and `sim_math` cross-target digest. |
| **Multi-tenant** | `TenantEntityIndex` prefixes ECS entity keys with `(Tenant, Uuid)` tuple bytes; `warden` isolates each tenant in its own `wasmtime::Store`. | `ecs::tests::two_tenants_with_identical_uuid_resolve_to_disjoint_entities`. |
| **Fastest** | `capture_descendants_many` cuts 10,000-root process polling from 10,000 table scans to 1 scan + BFS; pipe gate removes `RELEASE_POLL` sleeps. | 10,000-root snapshot p50/p95/p99 `< 15 ms` in `bench/`. |

- **Wave 1 (First-party `ELIMINATE` + `#841` bugfix).** Land `#841`, `#798`, `#814`/`#808`, `#851`, `#842` (retiring `thiserror`), `#837`, and `#799`.
- **Wave 2 (Cryptography & PKI).** Land `lgwks_std::ec` (`#794`, `#840`) followed by `lgwks_deps` `pki-x509` and `pdf-sign` (`#833`).
- **Wave 3 (Storefront boundaries).** Land `#849`/`#848`/`#838` (`sqlite` + `web-loco` + `proptest`), `#846`/`#854` (`warden-*` + `apple-vz`), and `#850`/`#836` (`mcp-remote` + `ui-preview`).
