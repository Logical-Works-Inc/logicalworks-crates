# Bot semantics on an ECS substrate

Continued in: [The one finding](bot-on-ecs-finding.md), [The design rule](bot-on-ecs-design-rule.md), [The GPUI assessment](bot-on-ecs-gpui.md), [The data plane](bot-on-ecs-data-plane.md), [The open decision](bot-on-ecs-decision.md), [Running the tests](bot-on-ecs-tests.md).

This page describes the mapping of the four-verb bot model onto an ECS
substrate, and the cost of that mapping. It assumes the async surface defined in
`docs/async-sdk-shape.md`, which states what a consumer of that surface is
required to reason about.

The question addressed here is different: **what substrate executes a `BotSpec`,
and does the four-verb model extend onto it naturally?**

Status: **implemented, and it is the only path.** Steps 0, 2 and 3 of §10 have
landed; `Bot` *is* the ECS bot: one bot, one builder and one executor, with no
feature flag and no parallel implementation. `bevy_ecs` is admitted
(`docs/bevy-admission.md`). The scheduler decision (§9, step 1) is taken, and the
module it chose is not built.

## 1. Why an ECS at all

The four verbs are already an ECS in disguise, and naming that buys three
things:

| Current type | ECS role | What it buys |
|---|---|---|
| `Cap` / `Auth` | `Resource`: the capability proof | One authority per world; no ambient singleton |
| `Observe` impls (`Endpoint`, …) | `Component` + a system that writes it | Heterogeneous sources without `Box<dyn>` |
| `Evaluate::check` | **change detection** (`Changed<T>`) | The condition is a tick comparison, not a re-derivation |
| `Execute::execute_action` | a system that writes the effect | Effects are data, not callbacks |
| `BotSpec`, `ChainSpec` | a `Schedule` built from deserialized data | The spec *is* the ordering declaration |
| `Bot::tick()` | `Schedule::run(&mut World)` | One tick is one step, manually driven |

The last row is the one that matters for manual stepping: `Bot::tick()` is
already a manual step, and `Schedule::run(&mut World)` is the same shape. There
is no framework-owned main loop to surrender control to: `App::update()` is
available and `App::run()` is never called.

## 2. What the spike measured

`bevy_ecs` 0.19.1, `default-features = false, features = ["std"]`. A two-source
bot with a capability resource, a `Changed<T>` condition, and a
`Commands`-issued effect, stepped five times by hand.

**The mapping holds.**

```text
diag: Changed<Status> matched: [2, 0, 2, 0, 2]
diag: Status values per tick: [[200,200],[200,200],[503,503],[503,503],[200,200]]
diag: spawn requests/tick   : [0, 0, 2, 0, 0]
total effects               : 2
ungranted world effects     : 0
flat-upstream effects       : 0
determinism                 : identical across rebuilds
```
- `Changed<T>` is a precise condition primitive: it matched on exactly the ticks
  where the observed value moved (ticks 2 and 4), not on every tick.
- The capability gate as a `Resource` correctly produced **zero** effects in a
  world without the grant, the equivalent of `Auth::check` denying.
- A flat upstream produced zero effects: a tick where nothing changed reports
  nothing changed. That is the property a naive "re-run every condition" loop
  does not have.
- Rebuilding the schedule and world from scratch reproduced the sequence exactly.
