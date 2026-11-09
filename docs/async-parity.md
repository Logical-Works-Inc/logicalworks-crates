# Async runtime parity

Continued in: [Closed capabilities](async-parity-closed.md), [What closed means](async-parity-closed-meaning.md).

This page compares the surface of `lgwks_bot::rt` against the third-party
runtime crates a consumer would otherwise reach for, measured against their
current exported surfaces rather than against advertised feature lists.

Status: audited 2026-09-20 for the 0.4.0 release; the third-party surface was
re-pinned on 2026-10-02. Three gaps were found and closed; two more were closed by
#152 (virtual time and task introspection, both in a deliberately different shape
— §4.1), and the two that remain open are named in §4.

## 1. Method

`lgwks_bot::rt` is a **facade over tokio**, so "parity with tokio" is the wrong
frame: the engine *is* tokio, reached through the `lgwks_deps` storefront so
that no other crate authors a `tokio` edge. The question worth asking is which
parts of tokio's surface the facade **exposes**, because a capability the facade
does not re-export is one a consumer cannot use without breaking the
single-entry rule.

The audit therefore compared the exported surface, not the engines. `async-std`
and `smol` are listed for shape: both are alternatives a consumer might pick
instead of the facade entirely, so a capability they have and this does not is a
reason to leave.

The matrix is pinned to **tokio 1.53.1**, **tokio-util 0.7.19**, **async-std
1.13.2** and **smol 2.0.2** (published crate versions, accessed 2026-10-02), and
compares surfaces only — it is not a benchmark. `async-std` is deprecated by its
own crate metadata in favour of `smol`; it is listed here only as shape. The
virtual-clock path named in §4 leans on `bevy_time`; Bevy's latest stable release
at that date is **0.19.1** (`0.20.0-rc.2` is a pre-release).

## 2. Matrix

| Capability | tokio | async-std | smol | `lgwks_bot::rt` |
|---|---|---|---|---|
| Owned runtime + builder | ✅ | ✅ | ✅ | ✅ `Runtime`/`Builder`/`Handle` |
| `block_on` | ✅ | ✅ | ✅ | ✅ `rt::runtime::block_on` |
| Spawn a `Send` task | ✅ | ✅ | ✅ | ❌ **by design** — `Supervisor::spawn`: bounded by an in-flight ceiling and reported, and it returns no handle to drop |
| Spawn non-`Send` | ✅ `LocalSet` | ✅ `spawn_local` | ✅ | ❌ **by design**, and neither escape route is a workaround: a non-`Send` future is awaited inline, or handed to a **caller-owned single-threaded driver**, not to `join_all_bounded` or `Supervisor::spawn` (both require `Send + 'static`) |
| Structured task set | ✅ `JoinSet` | ◐ `spawn`/`JoinHandle`, no set type | ◐ `spawn`/`Task`, no set type | ✅ `rt::task::JoinSet` |
| Off-thread blocking call | ✅ `spawn_blocking` | ✅ | ✅ | ✅ `lgwks_std::task::spawn_blocking` — not in `rt`, and it returns a future for the result rather than a handle |
| `yield_now` | ✅ | ✅ | ✅ | ✅ |
| Cancellation token | ✅ `tokio-util` | ❌ | ❌ | ✅ **closed 0.4.0** |
| `sleep`/`timeout`/`interval` | ✅ | ✅ | ✅ | ✅ `rt::time` |
| Virtual / paused test clock | ✅ `pause`+`advance` | ❌ | ❌ | ✅ **closed 0.9.0** — `rt::clock`, a *declared* clock; see §4.1 |
| `mpsc`/`oneshot`/`broadcast`/`watch` | ✅ | ✅ | ✅ | ✅ `rt::sync` |
| Bounded `mpsc` | ✅ | ✅ | ✅ | ✅ |
| `Mutex`/`RwLock`/`Semaphore`/`Notify`/`Barrier`/`OnceCell` | ✅ | ✅ | ✅ | ✅ |
| Reader/writer traits, `BufReader`, `copy` | ✅ | ✅ | ✅ | ✅ **closed 0.4.0** |
| `fs` | ✅ | ✅ | ✅ | ✅ |
| `net` TCP/UDP/Unix + `lookup_host` | ✅ | ✅ | ✅ | ✅ |
| `process` | ✅ | ✅ | ✅ | ✅ `Command` to describe, `Supervisor::spawn_process` to run — bounded, killed as a process group, reported |
| Signal streams | ✅ | ✅ | ✅ | ✅ |
| `select!`/`join!`/`try_join!` | ✅ | ✅ | ✅ (futures-lite) | ✅ crate root |
| Runtime shutdown with timeout | ✅ | ❌ | ❌ | ✅ `shutdown_timeout` |
| Shutdown without waiting | ✅ `shutdown_background` | ❌ | ❌ | ❌ minor |
| Move a blocking call off a worker | ✅ `block_in_place` | ❌ | ❌ | ❌ minor |
| Task id / introspection | ✅ `task::id` | ❌ | ❌ | ✅ **closed 0.9.0** — `Supervisor::snapshot`; see §4.1 |
| `#[main]` / `#[test]` attribute | ✅ | ✅ | ❌ | ❌ **by design** |
| `tracing` integration | ✅ feature | ❌ | ❌ | ✅ via `lgwks_std::trace` |

◐ means the capability is present only in the weaker shape named. `async-std`
and `smol` return a spawned, awaitable handle (`task::spawn`; `Task`) but export
no `JoinSet` that owns and aborts the whole set, so their "structured task set"
cells are not full matches (`docs.rs/async-std/1.13.2`,
`docs.rs/smol/2.0.2`). `smol`'s public modules at 2.0.2 are `channel`, `fs`,
`future`, `io`, `lock`, `net`, `process` and `stream` — no cancellation token —
so its `CancellationToken` cell was corrected from ✅ to ❌; only `tokio-util`
ships one.
