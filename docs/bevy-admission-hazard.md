# Bevy ECS admission: The hazard

Part of [Bevy ECS admission](bevy-admission.md).

## 4. The hazard that admission must be conditioned on

Measured on `bevy_ecs` 0.19.1 with the configuration used here. A system
that ran, matched its `Changed<T>` filter, and entered its effect branch
produced **zero** effects, with no panic, no error, and no diagnostic, because
`Commands` are not flushed unless `ApplyDeferred` is added explicitly. Deleting
one line took the count from 2 to 0 while changing nothing else in the output.
Even once flushed, the effect was first visible one tick *after* it was issued.

Two conditions follow, and they are conditions of admission rather than
suggestions:

1. Effects are expressed through **exclusive systems** (`&mut World`) at named
   points, not through the deferred command queue.
2. `Schedule::initialize()` and `ScheduleBuildSettings { ambiguity_detection:
   LogLevel::Error }` run at `Bot::build()` time, so a schedule that cannot be
   ordered deterministically is a refusal at build, not a silent misordering at
   tick.

Full evidence in `docs/bot-on-ecs.md` §2–§3.

**A third condition, found by implementing rather than by spiking.**
`Component: Send + Sync + 'static` is unconditional in `bevy_ecs` 0.19.1, and
there is no `non_send` feature to relax it. `NonSend` survives for **resources**
only. This collides with `lgwks_bot`'s deliberate non-`Send` verb erasure (its
futures are not `Send` on purpose, so that a domain may hold thread-local state)
and a `Box<dyn Any>` produced by that erasure cannot be a component.

Both workarounds were considered and one was taken:

- **Taken.** Keep the value in a `NonSend` resource and put a `Revision(u64)`
  marker component on the entity, bumped only when the value differs. The
  condition is still `Changed<Revision>`, a tick comparison, so the property
  the admission was for is preserved, and the crate's non-`Send` contract is
  untouched.
- **Rejected.** Tighten the erasure to `Box<dyn Any + Send + Sync>` and require
  `Send + Sync` on every observer's output. It would fit the substrate better and
  cost every existing and future observer a bound it does not otherwise need, to
  solve a problem the substrate introduced.

The cost of the choice is a `PartialEq` bound on an observer's `Output`: change
detection needs to tell "the same value again" from "a new value", and a type
that cannot be compared cannot be detected as changed.

## 5. What CodeGraph showed about adjacent components

The Bevy index (1,932 files, 51,020 nodes, 230,384 edges) was queried for the
mechanisms the three consumers need. Three findings changed the shape of this
admission.

**Adopted alongside `bevy_ecs`:**

- **`bevy_time`** — `Time<Virtual>` (`crates/bevy_time/src/virt.rs:75`) is a real
  virtual clock: `pause`, `set_relative_speed`, `max_delta`, `effective_speed`,
  and a `Time<Fixed>` overstep accumulator in `fixed.rs`. It was
  admitted so a test could run 10,000 ticks without touching the wall clock, and
  no workspace code consumes it yet ([the re-run](bevy-0.20-spike.md) §5).
- **`bevy_state`** — `NextState` (`state/resources.rs:181`), `State<S>`,
  `PreviousState`, `StateTransitionEvent`, `apply_state_transition`
  (`state/freely_mutable_state.rs:49`), plus `SubStates` and `ComputedStates`.
  This is a declared state machine with generational transitions and enter/exit
  schedules, which is the control-plane shape bors arrives at by hand
  (`docs/bot-on-ecs.md` §7). Adopting it means the transition is data rather
  than a chain of conditionals.

**Rejected after inspection:**

- **`bevy_asset`, for dependency tracking.** It has a dependency graph
  (`dependency_load_state`, `DependencyLoadState` in `src/server/mod.rs`), but it
  is an *asset loading* graph, tied to `AssetId` and the asset server. It is not
  a general value-dependency graph and using it as one would fight it.

**Absent, and this is the honest gap:**

**Bevy has no general-purpose invalidation graph.** Querying `invalidate` and
`Dependency` across all 62 crates returns only two things: the *schedule* DAG
(`crates/bevy_ecs/src/schedule/graph/mod.rs:23`) and the asset-loading graph
above. There is no "this derived value depends on that source value" structure
anywhere in the engine.

So the substrate provides the **primitive** (per-component `Tick` stamps and
`ComponentTicks`, with `set_if_neq` as the precise-notification path) and not
the **graph**. The drishti incremental derivation and the GPU framework's demand
closure must be built on top of `Changed<T>`, and that is real work this
admission does not do for them. Stating it plainly matters more than the
admission does: a consumer that assumes a dependency graph exists will not find
one.

## 6. Risks accepted

- **Pre-1.0 churn.** Bevy breaks between minor versions. The admission moved
  from `^0.19` to `^0.20` on 2026-10-22 ([the re-run](bevy-0.20-spike.md)). Every version bump is a
  reviewed change with the spike re-run, not a `cargo update`.
- **`bevy_ecs` is not minimizable further.** It unconditionally pulls
  `bevy_tasks`, `bevy_utils`, and `bevy_platform`. Those four plus the macro
  crates are the floor, and 89 packages on 0.20 is what that floor costs.
- **Two determinism properties remain this workspace's responsibility, not Bevy's.** There is
  no first-party `bevy_replay` or `bevy_determinism`; entity- and archetype-ID
  allocation is history-dependent; and `bevy_platform`'s `HashMap` has a fixed
  hasher but documented **arbitrary iteration order**. Anything emitted or
  compared must be sorted first.
