# Codebook: dependencies, banned constructs and evidence

Part of [`CODEBOOK.md`](../../CODEBOOK.md), sections 10 to 12.

## 10. Dependencies

Estate first, in this order. The **reuse ladder** (std → `lgwks_std` →
`lgwks_bot` → `lgwks_ast` → `lgwks_deps`) is an ownership order, and it is not
the dependency graph. The graph, read from the manifests, is:
```text
lgwks_std      (depends on nothing in this workspace)
lgwks_ast      → lgwks_std, optional (feature `tool`, `scan` and `legacy` readers)
lgwks_deps     → lgwks_std, and optionally lgwks_ast (feature `lgwks`)
lgwks_macros   → lgwks_std, lgwks_ast, lgwks_deps
lgwks_bot      → lgwks_std, and optionally lgwks_ast, lgwks_deps, lgwks_macros
drishti        → lgwks_std, and optionally lgwks_bot, lgwks_deps
```
So `lgwks_ast` sits late in the ladder and early in the graph, `lgwks_deps` depends
on `lgwks_ast` and never the reverse, and `lgwks_macros` and `drishti` are absent
from the ladder entirely. Reach for a capability in ladder order; do not read the
ladder as a build order.

- JSON is `lgwks_std::json` with `#[serde(crate = "lgwks_std::json::serde")]`.
  Do not declare `serde` directly.
- Async is `lgwks_bot::rt`. `lgwks_std::task` is sync-only. Do not author
  `tokio` or `futures` edges.
- A new third-party dependency is an optional, feature-gated edge of
  `lgwks_deps`, registered in `contract/APPROVED.toml`, never a direct edge and
  never a new top-level crate.
- Estate edges come from the registry (crates.io), not a git rev. A git pin
  cannot witness that it still matches a published release and silently drifts
  behind the estate's own published surface.
- A git pin is a recorded exception in `contract/PATCHES.toml`, with its `rev`
  ([`docs/patches.md`](../patches.md)). Exact resolved versions are pinned by `Cargo.lock`.
- If a capability exists in `std`, `lgwks_std`, `lgwks_bot`, or `lgwks_ast`, use
  it. Duplicating a parser, runtime, codec, HTTP client, ID type, time helper, or
  fs wrapper the estate already provides is a defect.

---

## 11. Banned constructs and their replacement

| Banned | Replacement |
|---|---|
| `tokio::sync::mpsc::unbounded_channel` | `lgwks_bot::rt::sync::mpsc::channel` (bounded) |
| `std::sync::mpsc::channel` | the bounded channel |
| `tokio::spawn` | `lgwks_bot::rt::task::spawn` inside a tracked scope |
| `std::thread::spawn` | `std::thread::Builder::spawn` with a joined handle, or `lgwks_bot` |
| `std::thread::sleep` | `std::thread::park_timeout` (sync) or `rt::time::sleep` (async) |
| `std::mem::uninitialized` | `std::mem::MaybeUninit` |
| `std::mem::zeroed` | `MaybeUninit`, or a typed constructor |
| `std::mem::forget` | break the cycle; use `Weak` |
| `std::rc::Rc::new_cyclic` | `Weak` back-pointers |
| `std::vec::Vec::remove(0)` | `VecDeque` for queue semantics |
| `String` errors in library code | typed error enums with `source()` |
| `unreachable!()` | a typed `Err` the caller can handle |
| struct expression on a `#[non_exhaustive]` type | the type's `pub fn new(...)` |
| `println!` / `eprintln!` in library code | `lgwks_std::trace` |
| `process::exit` outside a crash fixture | return the typed error |

---

## 12. What a green gate does not prove

A green compile, a green test suite, and green CI are first-party evidence of
exactly what they ran and nothing more. They do not prove:

- that the anti-patterns no lint expresses are absent;
- that a measurement environment exists where none is committed;
- that a migration is complete because its documentation says "canonical";
- that a scale, cost, availability, or superiority claim is established.

Where evidence is missing, say **unknown**, **not exercised**, or **not
independently verified**. Never substitute a plausible story. Self-authored
tests, fixtures, PR descriptions, docs, and model summaries are first-party
evidence only. Mergeable and green is not merge-safe.

This contract outranks velocity and model confidence.
