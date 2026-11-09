# Bot semantics on an ECS substrate: The data plane

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 8. The data plane: the frontier

A bot's canonical workload is an observe→evaluate→execute loop over a set of
sources, which is a crawler with an action attached. Four reference
implementations were studied (spider-rs, Scrapy, Heritrix, Nutch) plus Mercator's
1999 architecture, which is still the canonical shape.

**Frontier structure.** Mercator's design (per-host queues under a global
priority queue over hosts) is right, and for a reason worth stating: politeness
is a per-host property and priority is a global one, so one flat queue cannot
express both. A bounded per-host queue under a global host priority queue also
gives backpressure for free: a host whose queue is full stops being selected
rather than growing without limit.

**Politeness is a scheduling policy, not a fixed delay.** Scrapy's `AutoThrottle`
and Mercator both adapt; Mercator specifically uses priority decay plus
weighted-random selection rather than a fixed re-crawl interval. Note a
correction to the common summary of that paper: Mercator has **no
interval-adaptation rule**: the adaptive criteria live in US patent 6,263,364,
not the paper. Do not cite the paper for behaviour it does not describe.

**Determinism primitives worth adopting, none of which come from a crawler:**

- **Heritrix's comparator rule, stated plainly in `WorkQueue::compareTo`: *"at
  this point, the ordering is arbitrary, but still must be consistent/stable
  over time."*** Any comparator that can tie must be strengthened until it
  cannot. This is the rule of deterministic event ordering at every collection
  that is iterated, arrived at independently.
- **FoundationDB splits three RNG streams** — `deterministicRandom()`,
  `nondeterministicRandom()`, and `debugRandom()`, so that a debug draw cannot
  shift the main stream. It uses `boost::mt19937_64` rather than `std::`
  distributions specifically because those differ between libstdc++ and libc++,
  i.e. for cross-toolchain reproducibility of the stream. For this workspace: a
  log line, a metric, or a `HashMap` iteration must never perturb the scheduling
  stream.
- **Kafka's `read_committed` LSO** is the right shape for a replayed frontier: a
  replaying reader should see what has been **decided**, not what has been
  **written**.
- **Temporal's `GetVersion` rule**: a version marker must be called *forever*,
  even after the branch it guarded is deleted, because the marker event exists in
  history. The persistence corollary: never remove a tie-break field from a
  persisted record.

### 8.1 The finding that changes the design

**A single-bit seen-set is structurally insufficient, and no reference crawler is
durable-before-fetch.**

The two failure modes are asymmetric, which is why ordering alone cannot fix
them: fetch-then-mark loses the *fact of the fetch* (the URL is dropped and
unrecoverable without re-seeding), while mark-then-fetch loses the *fetch itself*.
A mark that means only "known" cannot be re-driven.

The fix is a **two-phase mark**: a durable `Pending` recorded *before* the fetch,
a durable `Done` after it, with recovery re-driving everything left `Pending`.
The seen-set gains a state machine rather than a flag.

This is not matching a reference implementation. It is exceeding all four:

| Crawler | Durability before fetch |
|---|---|
| Heritrix | `checkpointIntervalMinutes = -1`: **checkpointing is off by default**. The recovery journal writes through a 32 KiB `BufferedOutputStream` whose `writeLine` never flushes, so a hard kill loses the tail. `importRecoverFormat` catches `EOFException` with the comment *"expected in some uncleanly-closed recovery logs; ignore."* |
| Scrapy | `requests.seen` has **no `flush()` and no `fsync` anywhere**; `close()` only closes the handle. The OS page cache is the entire durability story. |
| Nutch | `generate.update.crawldb` is `false` by default, so nothing is marked at generate time. Worse, on `fetcher.timelimit` expiry `Fetcher::emptyQueues()` **drops the remaining queue without writing it**, not even as failed statuses. |
| Mercator | Paper-level; no durable-frontier claim to evaluate. |

Also worth copying from Heritrix: `Checkpoint.VALIDITY_STAMP_FILENAME = "valid"`.
A validity stamp is what makes a **torn checkpoint detectable** rather than
silently half-loaded, the failure mode `Bot::tick` would otherwise inherit.

### 8.2 Reconciling bors and the frontier

§7 says *checkpoint after the side effect, in a single write*. §8.1 says *record
`Pending` before the fetch*. These look opposed and are not; the difference is
where the durable log lives.

bors can write after the effect because its durable log **is the external
system**: the merge and push to GitHub are themselves the record, so there is
nothing to write first. A crawler has no such external log; the frontier is the
only record, so the intent must be durable before the effect.

The unifying rule: **where an external system is the durable log, checkpoint
after the effect and re-read truth from it. Where the local record is the only
log, it needs a state machine (`Pending` then `Done`) and recovery re-drives
the ambiguous middle.** The failure to avoid is identical in both cases: a record
that cannot distinguish "done" from "never started".
