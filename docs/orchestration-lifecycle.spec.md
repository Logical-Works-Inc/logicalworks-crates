# Orchestration lifecycle and Rust invariants

Continued in: [Cancellation, readiness and cleanup](orchestration-lifecycle-cancellation.spec.md).

Status: **the target; not an implementation receipt**. Baseline and tracking are
in [the task-first contract](declarative-orchestration.spec.md), and each LC row's
tests are named in [the acceptance page](orchestration-acceptance.spec.md).
The public API is small because the host owns these obligations once.

## One owner and independent facts

**LC-01.** One host reducer owns each run's transitions. Commands propose work;
observations and persisted acknowledgements advance state. Parallel workers
return typed facts, not arbitrary mutations of shared workflow state. For a
single-process host this can be one owner task. Distributed takeover requires
an actual store/security fencing mechanism, not a local mutex or TTL alone.

The report MUST preserve these orthogonal facts:

```text
Disposition = Completed | Blocked | Failed | Cancelled
EffectKnowledge = per-effect NotStarted | NotApplied | Applied | Unknown
Cleanup = Complete | Pending | Failed
Verification = Satisfied(evidence) | Unsatisfied(evidence) | Unmeasured(reason)
```
These are semantic categories, not mandated public enum spellings. A failed
workflow can still have applied or unknown effects. A cancelled task can have
pending cleanup. The public completion conversion must refuse to return a
verified success when any required effect or postcondition is unresolved.
Infrastructure errors after dispatch must carry a run/report reference and the
last known state, not erase occurrence uncertainty behind `Err`.

## Admission before work creation

**LC-02.** Reserve the relevant resource budget before starting an external
operation or materializing an expensive closure/payload. Bound active workers,
queued work, retained results, process output, event buffers and aggregate
artifact bytes independently. A limit on active tasks is not a memory bound
on the returned collection. A collecting operation must require a total bound;
large/unbounded sources use backpressured streams or artifact references.

Nested composition must not deadlock at concurrency one: a parent awaiting
children cannot occupy the execution permit all children need. Suspended
parents retain separately charged memory, not active compute permits. Resource
acquisition order must be fixed or atomic across multiple classes. Pipe drain,
cleanup and reconciliation work need reserved capacity so the main operation
cannot consume the resources necessary to finish itself.

**LC-03.** Default scheduling is local and bounded. Independent ready work
progresses without wave-wide head-of-line blocking. Conflicting writes acquire
explicit resource domains (for example one browser's focus/session epoch), not
one global lock over all runs. CPU work, model calls and I/O have separately
resolved limits. Fairness across runs must be tested at saturation; no inferred
unlimited fan-out from a declared DAG. An empty ready queue does not mean a
run finished while producers, unresolved events or held effects remain.

## Observation commits

**LC-04.** Commit an observation payload, its comparison baseline and its
cursor/revision atomically with respect to admission of work. A failed atomic
observation group commits no member's new payload and cannot mark it handled.

A detached fingerprint or remote ETag must not suppress an asynchronous poll:
the source may move A → B → A while that poll is pending, making an earlier A
key inconsistent with the returned B payload. A fast skip is permitted only
when an observer returns the value and a non-reused revision from one snapshot
operation, with its invalidation and overflow behavior declared. The
`Observe::revision` seam (#279, INV-BOT-155) is that contract, and a source that reports none is polled and compared on every tick.

## Attempts and external effects

**LC-05.** Before dispatch, record the exact operation input and attempt identity.
[#86](https://github.com/Logical-Works-Inc/logicalworks-crates/pull/86), merged 2026-09-21,
owns `EffectKey`; do not invent another. Its identity work is not closure
until the actual dispatcher, ledger, settlement and recovery paths consume it.

The required binding is run + action + attempt + definition revision + action
payload digest + environment identity + environment epoch. Logical invocation
identity, physical dispatch attempt and retry-budget counters are different
concepts. A transport retransmission preserves the provider's logical
idempotency key and exact semantic payload; a new attempt gets a new attempt
identity. Neither is inferred from vector position or debug `type_name`.
Checked exhaustion refuses instead of wrapping or saturating an identity.

**LC-06.** Settlement compares the full subject before changing state. Delivery
of evidence for attempt A is idempotent for A even after attempt B starts;
it cannot authorize B's retry. Contradictory evidence is reported. Evidence of
non-occurrence is not renewed policy authority or proof of transient failure.
A permanent refusal needs an explicit authorized repair/new decision, not an
unbounded cycle of `NotApplied` budget resets. Validate evidence provenance,
not merely its structural fields.

A durable store operation must atomically bind expected state/owner epoch,
append the next event and reserve dispatch identity. An append acknowledgement
states its persistence level. At startup, attempted-without-outcome means
Unknown. Persisting intent cannot make an arbitrary non-idempotent endpoint
exactly-once; reconciliation or a supported idempotency protocol is still
required. A crash before recording a response may require holding work even
when the effect succeeded.

**LC-07.** Retry policy is a total typed decision over dispatch certainty,
transience, idempotency support, remaining budget and retry timing. Do not parse
human cause strings. Honor typed rate-limiting information. Choose one retry
owner; count adapter/network retries against the effective aggregate budget.
Do not retry an entire workflow to reach one unfinished successor. Retries,
model-output repair and replanning consume separately reported budgets.
