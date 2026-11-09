# Bot semantics on an ECS substrate: The one finding

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 3. The one finding that shapes the design

**`Commands` do not flush unless a sync point is ordered after the writer.**

Deleting the single line `schedule.add_systems(ApplyDeferred)` takes `total
effects` from 2 to **0**, and *nothing else in the output changes*: the system
still ran, `Changed<T>` still matched, `spawn requests/tick` still reported
`[0,0,2,0,0]`. The effect simply never materialized. No panic, no error, no
diagnostic: a bot whose Execute verb silently does nothing.

The reason is sharper than "the sync point was missing", and a second spike
measured it as a four-way control, because the first phrasing points at the wrong
remedy. The setting `ScheduleBuildSettings::auto_insert_apply_deferred` **defaults to
`true`**; it inserts the sync point only where some system is ordered *after* the
deferred writer. Repeat the effect three times over five manually stepped ticks,
so a correct run records 3:

| Schedule | Effects |
|---|---|
| writer is the last system, no sync point | **0**: the queue is dropped at the end of `Schedule::run` |
| writer, then an unrelated successor (auto-insert) | 3 |
| writer, then `add_systems(ApplyDeferred)`: unordered | **2** |
| `(writer, ApplyDeferred).chain()`: ordered | 3 |

Row 3 is the one worth staring at. Adding `ApplyDeferred` *is* the remedy the
failure message suggests, and it still loses an effect. An unordered sync
point can run before the writer and defer the effect by a tick, which on a
fixed-length run drops the last turn entirely. The hazard is not "the sync point
is absent"; it is **"a deferred effect lands at a time decided by the schedule
graph rather than by the caller"**, and the only fix that holds is to not defer.

Two consequences, and they are both requirements rather than observations:

1. **The bot must not express effects through `Commands`.** An effect written
   through the deferred queue is visible at a schedule-graph-dependent moment.
   The spike shows the second half of the hazard too: even once flushed, the
   effect is first visible **one tick after it was issued** (requested on tick 2,
   counted on tick 3). Use an **exclusive system** (`&mut World`) at a named
   point, where the ordering is the caller's declaration rather than a
   consequence of graph construction.
2. **A missing sync point must be a build error, not a silent no-op.** Whatever
   schedule a `BotSpec` compiles to, `Schedule::initialize() -> Result<_,
   ScheduleBuildError>` and `ScheduleBuildSettings { ambiguity_detection:
   LogLevel::Error }` must both be run at `Bot::build()` time. An ambiguous
   schedule is currently a silent ordering bug; at `LogLevel::Error` it becomes a
   refusal. This is the ECS form of an existing habit in this workspace: failing
   at `build()` rather than at `tick()`.

Third finding, minor but worth recording: `World::entities().len()` is **not** a
live-entity count. `World::new()` alone reports 4, every `insert_resource` grows
it, and a `spawn` that reuses a free slot does not change it. Counting requires a
query. Anything that reports "N bots running" must count by query.
