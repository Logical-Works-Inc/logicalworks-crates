# System and ecosystem capability gaps: first-party elimination

This specification defines the first-party (`ELIMINATE`) designs for `lgwks_std`, `lgwks_bot`, `lgwks_macros`, and `lgwks_deps` capability gaps (`#798`, `#814`, `#808`, `#851`, `#842`, `#837`, and `#799`). It is the target, and the last column below says where each row stands. [ADR 0014](adr/0014-std-and-deps-capability-gaps.md) says what landed. Storefront boundary specifications (`#841`, `#849`, `#848`, `#794`, `#840`, `#833`, `#846`, `#854`, `#850`, `#836`, and `#838`) continue in [part two](std-deps-storefront-gaps.spec.md), and the governing architectural decision is [ADR 0014](adr/0014-std-and-deps-capability-gaps.md).

## 1. Admission classification

Every capability gap resolves to Rung 5 (`ELIMINATE` in-tree with zero new third-party dependencies) or Rung 4/8 (`BOUNDARY` optional feature on `lgwks_deps` with a schema-3 `[approved.retire]` mortgage):

| Issues | Capability | Rung | Owner surface | State |
|---|---|---|---|---|
| `#798` | Single process-table snapshot across `N` roots | Rung 5 (`ELIMINATE`) | `lgwks_std::process` | shipped |
| `#814`, `#808` | Inherited-FD passing and pipe start gate | Rung 5 (`ELIMINATE`) | `lgwks_std::process`, `lgwks_bot::rt` | #808 shipped, #814 open |
| `#851` | Full FoundationDB tuple codec and ECS `EntityUuid` | Rung 5 (`ELIMINATE`) | `lgwks_std::id::tuple`, `lgwks_bot::ecs` | codec shipped, ECS half removed and open |
| `#842` | Typed `std::error::Error` derive without `thiserror` | Rung 5 (`ELIMINATE`) | `lgwks_macros`, `lgwks_ast` | shipped |
| `#837` | Deterministic IEEE 754 transcendental float math | Rung 5 (`ELIMINATE`) | `lgwks_std::math` | removed and open |
| `#799` | Local RustSec advisory-db evaluation | Rung 5 (`ELIMINATE`) | `lgwks_deps::advisories` | shipped |

## 2. Multi-root process-table snapshot (`#798`)

`lgwks_std::process::capture_descendants_many(roots: &[i32]) -> io::Result<BTreeMap<i32, DescendantSet>>` snapshots descendant trees for up to `MAX_CAPTURE_ROOTS` (`16_384`) roots in one pass.

- **Validation.** Any `root <= 0` or `roots.len() > 16_384` returns `io::ErrorKind::InvalidInput` before issuing a syscall. An empty slice returns `Ok(BTreeMap::new())` with zero syscalls.
- **Single table read.** On macOS (`sysctl(KERN_PROC_ALL)`), Windows (`CreateToolhelp32Snapshot`), and Linux when `roots.len() > 32` or `/proc/<pid>/task/<tid>/children` is absent, `parent_pairs()` runs once to build `BTreeMap<i32, Vec<i32>>`.
- **Overlapping roots.** Each root traversal borrows child slices rather than removing entries, so when root `B` is a descendant of root `A`, both receive complete `DescendantSet` values bounded by `MAX_CAPTURED_DESCENDANTS` (`4_096`) and `MAX_CAPTURED_DEPTH` (`64`).
- **Falsifier.** `sim_capture_many` verifies across 2,000 seeded 10,000-root process graphs that `capture_descendants_many` performs one table read and matches per-root `capture_descendants` over a frozen table.

## 3. Inherited file descriptors and pipe start gates (`#814`, `#808`)

`ProcessSpec` and `GatedIdentifiedSpawn` replace shell `SIGSTOP` polling with kernel descriptor inheritance and an anonymous pipe start gate.

- **Inherited descriptors (`#814`).** `ProcessSpec::pass_fd(&mut self, child_fd: u32, fd: OwnedFd)` attaches up to `MAX_INHERITED_FDS` (`8`) descriptors at `child_fd >= 3`, refusing duplicate target numbers or `child_fd < 3` with `io::ErrorKind::InvalidInput`.
- **Anonymous pipe start gate (`#808`).** `GatedIdentifiedSpawn` opens a `CLOEXEC` pipe before spawn. The child blocks on a 1-byte read from the inherited gate descriptor before `exec`.
- **Release and fail-closed drop.** `GatedIdentifiedSpawn::release` verifies `identify_process(pid)` against `leader.started()`, writes `b"R"`, and closes the write end. If the coordinator crashes or drops `GatedIdentifiedSpawn` without calling `release`, the kernel closes the write end, the child reads `EOF` (`0` bytes), and exits `99` without executing the target binary.
- **Falsifier.** `tests/it/process_ownership.rs` holds the proof: zero payload execution on coordinator death and zero timer polling on release.

## 4. FoundationDB tuple layer and ECS identity surface (`#851`)

[ADR 0010](adr/0010-estate-uuid-replaces-the-uuid-crate.md) shipped `Uuid::now_v7` and UUID tuple code `0x30` while declining `serde` on `Uuid`. `lgwks_std::id::tuple` completes the wire format and `lgwks_bot::ecs` binds it to entities:

| Typecode | `Element<'a>` variant | Lexicographic wire encoding |
|---|---|---|
| `0x00` | `Null` | `0x00` at top level; `0x00 0xFF` inside a nested tuple (`0x05`) |
| `0x01` / `0x02` | `Bytes` / `String` | Code + bytes with `0x00` escaped as `0x00 0xFF` + terminating `0x00` |
| `0x05` | `Tuple(Vec<Element<'a>>)` | `0x05` + recursive elements (nulls escaped) + `0x00` (`MAX_TUPLE_DEPTH = 32`) |
| `0x0c`..`0x1c` | `Int(i64)` | `0x14` for `0`; `0x14 + n` + `n` big-endian bytes for `+`; `0x14 - n` + one's-complement bytes for `-` |
| `0x20` / `0x21` | `F32(f32)` / `F64(f64)` | Code + IEEE 754 big-endian bytes (`XOR 0xFF` all bytes if negative, else `XOR 0x80` high byte) |
| `0x26` / `0x27` | `Bool(bool)` | `0x26` (`false`), `0x27` (`true`) |
| `0x30` | `Uuid(Uuid)` | `0x30` + 16 raw network-order bytes |
| `0x32` / `0x33` | `Versionstamp80` / `96` | `0x32` + 10 bytes / `0x33` + 12 bytes |

- **Bounds and ECS index.** `pack_elements` and `unpack_elements` enforce `MAX_TUPLE_DEPTH = 32`, `MAX_TUPLE_ELEMENTS = 1_024`, and `MAX_TUPLE_KEY_BYTES = 10_000`. `lgwks_bot::ecs` adds `EntityUuid(pub Uuid)` and `TenantEntityIndex` keyed by `(Tenant, Uuid)` packed as `0x02 <tenant> 0x00 0x30 <uuid>`.
- **Falsifier.** `id::sim_tuple` (`sim_tuple_full_all_twelve_typecodes_pack_and_unpack_correctly`, `sim_fdb_tuple_element_lexicographical_bijection`) checks seeded compound keys against a model of the wire format and pairwise lexicographic order preservation.

## 5. Error derive (`#842`), float math (`#837`), and RustSec evaluator (`#799`)

- **Typed `Error` derive (`#842`).** `lgwks_macros::Error` (parsed in `lgwks_ast::error_derive` over `proc_macro2` without `syn`) supports `#[error("...")]`, `#[error(transparent)]`, `#[from]`, and `#[source]`. Landing it migrates `lgwks_ast` off `thiserror` and deletes `thiserror` from `contract/APPROVED.toml`.
- **Deterministic IEEE 754 float math (`#837`).** `lgwks_std::math` (default-on in `core`, zero dependencies, `#[forbid(unsafe_code)]`) implements `sqrt`, `cbrt`, `exp`, `ln`, `pow`, `sin`, `cos`, and `atan2` (plus `f32` counterparts) via `to_bits`/`from_bits` Cody-Waite reduction and minimax polynomials with no host `libm` FFI or target-dependent FMA contraction.
- **RustSec advisory evaluator (`#799`).** `lgwks_deps::advisories` walks a local `advisory-db` checkout (`crates/<pkg>/RUSTSEC-*.md`), parses frontmatter, hashes all read files with FNV-1a-128 (`hash` is not on this crate's normal edge), and evaluates `Cargo.lock` packages against `patched`, `unaffected`, and `withdrawn` SemVer ranges with zero new crate dependencies.
