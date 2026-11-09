# Bot semantics on an ECS substrate: Running the tests

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 11. Running the substrate's tests

A substrate whose tests never run is a substrate whose guarantees are claims, so
the tests run under the ordinary workspace gate:

```sh
cargo nextest run -p lgwks_bot --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
```
There is no feature flag. The substrate is how `Bot` executes, so the ordinary
workspace gate compiles and runs its tests. A default-off flag was tried first and rejected: it would have left the
substrate unexercised, unowned, and free to rot.

## 12. One path (Decision, 2026-09-20)

The first landing made the substrate a **default-off feature**: `EcsBot` beside
`Bot`, compiled only when a caller asked. That was corrected. A default-off
parallel path is how work fails to carry forward: nothing exercises it, the gate
never compiles it, and it rots into a second opinion nobody chose.

What changed:

- **The `ecs` feature is gone.** `lgwks_bot` depends on the storefront's
  `bevy-ecs` unconditionally. The storefront's own default selects no feature
  (ADR 0003), but a *consumer* states its opinion, and this crate's opinion is
  that the ECS substrate is how a bot executes.
- **The second builder is gone.** `EcsBot`, `EcsBuilder` and `EcsObserveBuilder`
  are now `Bot`, `BotBuilder` and `ObserveBuilder`, re-exported from a private
  module. There is no other way to build or run a bot.
- **The workspace suite runs it.** No flag, no separate command.
- **Two behaviours changed, and both are pinned by tests rather than noted:**
  - A tick is now **all-or-nothing on the poll side**: the observe system polls
    every source and commits the whole observation before any effect runs, so a
    tick whose *poll* fails fires *nothing*. The previous contract fired the
    chains declared before the failure. An *action* failure is the other case
    and unchanged: the effects before it already ran and are live.
  - A condition is evaluated on **transition**, not on every tick. The bot acts
    when the value moves. A condition that must hold continuously belongs on a
    domain that models it, not on change detection.
- **Deciding and doing are two systems.** `observe_fold` commits and detects,
  `fire_plan` records the effect program as an ordered list of steps, and the
  driver then awaits those steps in order (`EcsBot::run_steps`). The plan is
  fixed before any effect runs, which is what makes the order a property of the
  declaration rather than of when a future happened to be polled. It also gives
  a condition failure a defined position: the steps the walk cleared before the
  failing condition are still run, and the tick reports the failure.
- **`tick` and `tick_async` are the two adapters over that one tick.**
  One, `tick_async`, awaits the phases on the caller's executor. The other, `tick`, drives them
  with `lgwks_std::task::block_on`, and refuses with
  `BotError::TickInsideRuntime` when an async runtime already owns the calling
  thread, because parking that thread starves the reactor the bot's own verbs
  register with. There is no third path and no feature flag between them.
- **Bounded concurrent polling was restored, not dropped.** The first ECS
  `observe` polled sources one at a time, and `tick_polls_sources_concurrently`
  caught it, a real regression the collapse introduced. Polls run in bounded
  waves of `MAX_IN_FLIGHT_POLLS`, exactly as before; results are still collected
  in declaration order, so determinism is unaffected.

`--no-default-features` remains a real build (no tokio, no drivers), and it does
now pull `bevy_ecs`. That is the honest cost of one path rather than two.
