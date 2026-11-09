# Bevy ECS admission

Continued in: [The hazard](bevy-admission-hazard.md).

This page records the admission of four Bevy crates into the `lgwks_deps`
storefront, the alternatives measured against it, and the conditions attached to
the decision.

Register entries: `contract/APPROVED.toml`, `bevy_ecs`, `bevy_app`, `bevy_time`,
`bevy_state`, all `tier = "boundary"`, `owner = "lgwks_deps"`, approved
2026-09-20. Storefront features: `bevy-ecs`, `bevy-app`, `bevy-time`,
`bevy-state`.

Status: admitted, and now consumed. `lgwks_bot::ecs` is the first
implementation (see `docs/bot-on-ecs.md` §10–§11 for what it lands and, just
as importantly, what it does not).

## 1. Why one substrate and not three

Three consumers in the constellation need the same three primitives:

| Consumer | Needs |
|---|---|
| `lgwks_bot` (four-verb actors) | stable identity per source, "run this when that value changed", declared action ordering |
| drishti (incremental derivation) | invalidate only what a change affects; reuse valid work |
| GPU framework (`gpu-framework-first-pass`) | *"Changed demand: invalidate affected dependencies; reuse only valid keyed work"*; *"No demand: sleep"* |

The GPU framework's own architecture document states the requirement as an
intent-preservation invariant and a six-step scheduling policy in which step 2 is
literally *"invalidate affected dependencies; reuse only valid keyed work."*
That is change detection. The three consumers were separately re-deriving it;
admitting one substrate is cheaper than three approximations of it.

**What the workspace already had, and why it is not enough.** `lgwks_std::task`
is a zero-dependency executor with the property this workspace values most: no
`spawn`, so no task can leak. But it is deliberately
single-threaded, non-incremental, and has no query engine. It can await a bounded
set of futures and nothing more. It stays exactly as it is.

## 2. Measured cost

Taken from this workspace, not estimated:

```text
cargo tree -p lgwks_deps --features bevy-ecs --prefix none | sort -u | wc -l
→ 60 packages
```
```text
bevy crates pulled:
  bevy_ecs  bevy_ecs_macro_logic  bevy_ecs_macros  bevy_macro_utils
  bevy_platform  bevy_ptr  bevy_tasks  bevy_utils
```
`bevy_reflect` is **not** in the compiled tree (`grep -c bevy_reflect` → 0). It
appears in `Cargo.lock` (Cargo locks optional packages regardless of feature
activation), which is why the lockfile shows 15 bevy entries while only 8
compile. Reading the lock as the cost would overstate it by nearly half. On 0.20
the count is 89 packages ([the re-run](bevy-0.20-spike.md) §4).

An independent measurement of `bevy_ecs` + `bevy_app` with `default-features =
false` on an M5 Pro: 64 lockfile packages, **7–9 s cold build**, 131 MB release
`target/`. No renderer, no windowing, no audio, and no GPU driver in the graph.

`bevy_ecs` requires rustc 1.95; the toolchain for this workspace is pinned to 1.99.1
(`rust-toolchain.toml`), so there is no MSRV conflict.

## 3. What was deliberately excluded

- **`bevy_reflect`.** It is the expensive half of `bevy_ecs`'s default feature
  set. This workspace does not need runtime type reflection, and the measurement
  above confirms it is genuinely absent rather than merely untested.
- **`multi_threaded`.** `MultiThreadedExecutor::can_run` admits any system whose
  conflicting-system bitset is disjoint from the running set, so the relative
  order of two *non-conflicting* systems is not fixed. A bot polling sources
  gains no throughput from parallel scheduling and would pay for it in
  reproducibility. `SingleThreadedExecutor` is set explicitly per schedule, at
  the point the schedule is built.
- **`bevy_render` and everything above it.** The GPU framework chose wgpu as its
  only foundation. Bevy's renderer is also wgpu-based, but adopting it would
  import an opinionated render graph, an asset pipeline, and a shader
  permutation system that the framework's architecture explicitly declines to
  inherit. Rendering stays the GPU framework's decision, not this crate's.
- **`bevy_asset`, `bevy_scene`, `bevy_remote`.** Two were evaluated and deferred
  in §5.
