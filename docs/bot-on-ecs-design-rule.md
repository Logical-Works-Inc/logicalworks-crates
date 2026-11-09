# Bot semantics on an ECS substrate: The design rule

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 4. The design rule the spike taught

**Identity and mutable state must be separate components.**

The first version fused `url` and `status` into one `Endpoint` component. Every
poll then marked the whole component changed, so `Changed<Endpoint>` degenerated
to "always true" and the condition stopped meaning anything. Splitting into
`Endpoint` (identity, never mutated after spawn) and `Status` (the observed
value, the only thing the observer writes) is what makes change detection a real
condition primitive.

This generalizes into a rule for every domain in the `lgwks_bot` crate:
**a component is either identity or observed value, never both.** The corollary
is the Bevy guidance that variation belongs in an *enum payload* component, not
in a bespoke component set per bot: archetype count stays bounded and queries
stay linear.

**Amendment, measured while implementing step 2.** The observed value cannot be
the component the spike used. That spike's `Status` was a `Copy` `u16`; a real
observer produces `Box<dyn Any>` from the verb erasure, and `bevy_ecs` 0.19.1
declares `Component: Send + Sync + 'static` unconditionally: there is no
`non_send` feature to relax it (`NonSend` survives for **resources** only, which
is why `World::insert_non_send` exists).

So the value lives in a `NonSend` resource and the component is a `Revision(u64)`
marker the observe system bumps **only when the value differs**. That
keeps §4's property (the condition is still a tick comparison, not a
re-derivation) while keeping `lgwks_bot`'s non-`Send` contract intact rather
than tightening it to fit the substrate.

The cost is a `PartialEq` bound on an observer's `Output` in `Bot::builder`. It
is semantic, not incidental: this substrate's condition *is* change detection,
and a value that cannot be compared cannot be detected as changed. Every shipped
domain's observed state carries the derive.

The alternative was rejected: tightening the erasure to `Box<dyn Any + Send +
Sync>` would fit the substrate better and charge every observer a bound it does
not otherwise need, to solve a problem the substrate introduced.

### 4.1 Change ticks: the second change-detection path, and who uses it

The `Changed<Revision>` filter answers "did this source move?" for a chain the substrate
already holds a value for. The `Observe::revision` call answers it *earlier* — before the
poll, on the calling thread — so a source whose value has not moved costs one
vtable call and one `u64` comparison instead of a future, a boxed value and a
`PartialEq`:

| source | reports a revision? | why |
|---|---|---|
| `net::endpoint` | **no** — value comparison | the poll is an HTTP `GET`; it is what would discover that anything moved, so there is nothing to ask first |
| `fs::path` | **no** — value comparison | the poll is a `stat`; the metadata *is* the revision and reading it is the work being skipped |
| `data::json_store` | **no** — value comparison | the poll reads the whole file |
| `gh::pr_snapshot` | **no** — value comparison | the poll is a GraphQL snapshot |
| `sys::process` | **no** — value comparison | polling a one-shot process *is* dispatching it; there is no cheaper question |
| `chat::slack_message`, `chat::http_webhook` | **no** — value comparison | unbinding seams that refuse until a transport is bound |
| `inspect::subject` | **no** — value comparison | the poll reads the artifact and walks it |

**Every shipped source takes the comparison path, and that is the correct answer
rather than an omission.** A revision only pays when something *other than the
poll* maintains it: a subscription, a socket, a filesystem watcher, or a counter
another component already holds. A source whose poll is how it learns everything
about the world has no revision to report, because asking it first asks the
question the poll answers. The seam is for the sources a caller writes — the ones
whose world is pushed at them — and `bench/`'s source is the one in this
repository that can take it honestly, because its value is `tick / period` and the
counter is already in hand.

Two rules make the path safe, and both are the fingerprint-commit rule (INV-BOT-5,
#99) with a change tick in place of a digest:

1. **A revision is committed together with its value, or not at all.** The commit
   happens in the one arm of `observe_fold`'s second pass that put a new value in
   the observation slot. A poll that failed, was cancelled at its deadline, or
   produced a value equal to the one already held commits nothing — so the next
   tick asks again instead of treating a failed read as "nothing changed".
2. **A revision that moved is never a skip.** The comparison is equality only, so
   a revision that wraps (`u64::MAX` → 0) or regresses is a *different* revision
   and is therefore a change. There is no ordering for a wrap to invalidate.

A revision that moves while the value does is the one broken promise, and it is
deliberately the cheap direction: the source is polled, the value compares equal,
nothing commits, and the substrate asks again next tick. Committing the revision
anyway would be believing a source that has already lied once, which is how a
later real change gets skipped.

## 5. Determinism on this substrate

Bevy supplies the mechanism but not the guarantee, and it is worth being exact
about which is which.

- **`ExecutorKind::SingleThreaded`, set explicitly per schedule.** This is the
  single highest-leverage decision. The multithreaded executor's `can_run` admits
  any system whose conflicting-system bitset is disjoint from the running set, so
  which of two non-conflicting systems runs first is not fixed. For a bot polling
  sources, parallel scheduling buys throughput the workload does not need and
  costs interleaving reproducibility.
- **Ambiguity detection is opt-in** and defaults to `LogLevel::Ignore`. Turn it
  to `Error`. It runs at schedule build, not at runtime.
- **`Time<Virtual>` as the clock root**, with `Time<Fixed>`'s overstep accumulator
  providing the tick. `Time<Virtual>` supports `pause()`, `set_relative_speed()`,
  and `set_max_delta()`, so a test can run 10,000 ticks without touching
  wall-clock. The crate's own answer since #152 is the declared
  `rt::clock::Clock` (`docs/async-parity.md` §4.1).
- **Not supplied, and must not be assumed:** there is no first-party
  `bevy_replay` or `bevy_determinism`. Entity-ID and archetype-ID allocation are
  history-dependent, and `bevy_platform`'s `HashMap` has a fixed hasher:
  reproducible *hashing*, documented **arbitrary iteration order**. Sort
  deterministically before emitting or comparing anything. Determinism remains
  the responsibility of this workspace.
