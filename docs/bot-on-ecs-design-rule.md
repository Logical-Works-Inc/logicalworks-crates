# Bot semantics on an ECS substrate: The design rule

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 4. The design rule the spike taught

**Identity and mutable state must be separate components.**

The first version fused `url` and `status` into one `Endpoint` component. Every
poll then marked the whole component changed, so `Changed<Endpoint>` was always
true and the condition stopped meaning anything. Splitting it into `Endpoint`
(identity, never mutated after spawn) and `Status` (the observed value, the only
thing the observer writes) makes change detection a real condition primitive.

This becomes a rule for every domain in `lgwks_bot`: **a component is either
identity or observed value, never both.** The corollary is the Bevy guidance that
variation belongs in an *enum payload* component, not a bespoke component set per
bot. Archetype count stays bounded and queries stay linear.

**Amendment, measured while implementing step 2.** The observed value cannot be
the component the spike used. That spike's `Status` was a `Copy` `u16`. A real
observer produces `Box<dyn Any>` from the verb erasure, and `bevy_ecs` 0.19.1
declares `Component: Send + Sync + 'static` unconditionally. No `non_send`
feature relaxes it: `NonSend` survives for **resources** only, which is why
`World::insert_non_send` exists.

So the value lives in a `NonSend` resource, and the component is a
`Revision(u64)` marker the observe system bumps **only when the value differs**.
That keeps §4's property (the condition is still a tick comparison, not a
re-derivation) and keeps `lgwks_bot`'s non-`Send` contract, not tightened to fit
the substrate.

The cost is a `PartialEq` bound on an observer's `Output` in `Bot::builder`. It
is semantic, not incidental: this substrate's condition *is* change detection,
and a value that cannot be compared cannot be detected as changed. Every shipped
domain's observed state carries the derive.

The rejected alternative: tighten the erasure to `Box<dyn Any + Send + Sync>`.
It fits the substrate better, and charges every observer a bound it does not
need, to solve a problem the substrate made.

### 4.1 Change ticks: the second change-detection path, and who uses it

The `Changed<Revision>` filter answers "did this source move?" for a chain the
substrate already holds a value for. The `Observe::revision` call answers it
*earlier*, before the poll, on the calling thread. A source whose value has not
moved then costs one vtable call and one `u64` comparison, not a future, a boxed
value and a `PartialEq`:

| source | reports a revision? | why |
|---|---|---|
| `net::endpoint` | **no**: value comparison | the poll is an HTTP `GET`; it is what would discover that anything moved, so there is nothing to ask first |
| `fs::path` | **no**: value comparison | the poll is a `stat`; the metadata *is* the revision and reading it is the work being skipped |
| `data::json_store` | **no**: value comparison | the poll reads the whole file |
| `gh::pr_snapshot` | **no**: value comparison | the poll is a GraphQL snapshot |
| `sys::process` | **no**: value comparison | polling a one-shot process *is* dispatching it; there is no cheaper question |
| `chat::slack_message`, `chat::http_webhook` | **no**: value comparison | unbinding seams that refuse until a transport is bound |
| `inspect::subject` | **no**: value comparison | the poll reads the artifact and walks it |

**Every shipped source takes the comparison path, and that is correct, not an
omission.** A revision pays only when something *other than the poll* maintains
it: a subscription, a socket, a filesystem watcher, or a counter another
component holds. A source that learns everything through its poll has no
revision to report, because asking first asks the question the poll answers. The
seam is for the sources a caller writes, the ones whose world is pushed at them.
`bench/`'s source is the one here that can take it, because its value is
`tick / period` and the counter is already in hand.

Two rules make the path safe. Both are the fingerprint-commit rule (INV-BOT-5,
#99) with a change tick in place of a digest:

1. **A revision is committed with its value, or not at all.** The commit happens
   in the one arm of `observe_fold`'s second pass that put a new value in the
   observation slot. A poll that failed, was cancelled at its deadline, or
   produced a value equal to the one held commits nothing. The next tick asks
   again and never reads a failed read as "nothing changed".
2. **A revision that moved is never a skip.** The comparison is equality only, so
   a revision that wraps (`u64::MAX` → 0) or regresses is a *different* revision,
   and so a change. There is no ordering for a wrap to break.

A revision that stays still while the value moves is the one broken promise, and
it fails the cheap way. The source is polled, the value compares equal, nothing
commits, and the substrate asks again next tick. Committing the revision anyway
would trust a source that already lied once, which is how a later real change
gets skipped.

## 5. Determinism on this substrate

Bevy supplies the mechanism but not the guarantee. Be exact about which is which:

- **`ExecutorKind::SingleThreaded`, set explicitly per schedule.** This decision
  matters most. The multithreaded executor's `can_run` admits any system whose
  conflicting-system bitset is disjoint from the running set, so the order of two
  non-conflicting systems is not fixed. For a bot polling sources, parallel
  scheduling buys throughput the workload does not need, and costs interleaving
  reproducibility.
- **Ambiguity detection is opt-in** and defaults to `LogLevel::Ignore`. Set it to
  `Error`. It runs at schedule build, not at runtime.
- **`Time<Virtual>` as the clock root**, with `Time<Fixed>`'s overstep accumulator
  providing the tick. `Time<Virtual>` supports `pause()`, `set_relative_speed()`
  and `set_max_delta()`, so a test can run 10,000 ticks with no wall clock. The
  crate's own answer since #152 is the declared `rt::clock::Clock`
  (`docs/async-parity.md` §4.1).
- **Not supplied, so do not assume it:** there is no first-party `bevy_replay` or
  `bevy_determinism`. Entity-ID and archetype-ID allocation depend on history,
  and `bevy_platform`'s `HashMap` has a fixed hasher: reproducible *hashing*,
  documented **arbitrary iteration order**. Sort before emitting or comparing
  anything. Determinism stays this workspace's job.
