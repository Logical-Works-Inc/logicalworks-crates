# Orchestration lifecycle and Rust invariants: Cancellation, readiness and cleanup

Part of [Orchestration lifecycle and Rust invariants](orchestration-lifecycle.spec.md).

## Cancellation, readiness and cleanup

**LC-08.** Cancellation stops new admission and propagates through owned work.
`run` and explicitly persistent `submit` have distinct lifetimes. Dropping a
scoped waiter requests cancellation; the installed host continues to own
cleanup until drain or an explicitly reported cleanup failure. Merely dropping
a result view may not silently convert scoped work into persistent work.

Drop cannot await, and Rust does not guarantee destructors run. Never make
memory safety depend on final polling or destructor execution. Cooperative
cancellation cannot preempt a non-yielding callback. Isolate work requiring
forcible limits in an owned process/sandbox. A shutdown timeout may bound how
long the caller waits, but cannot truthfully certify that uncooperative work
stopped. Expose pending cleanup and deny reuse of still-owned resources.

**LC-09.** Starting a resource returns typed readiness, not a PID and a sleep.
Startup failure belongs to the initiating call; later failure is attributed to
the same resource and propagates to its dependants. A readiness token carries
resource identity and generation and becomes unusable after close/replacement.
A bound listener or completed browser handshake, not elapsed time, establishes
readiness. Duplicate/stale readiness messages are rejected.

**LC-10.** Create the child-process owner immediately after successful OS spawn,
before scheduling its management future. It owns all descendants that the
backend claims to control. Normal leader exit is not proof the containment is
empty. Group signalling alone is not containment against `setsid`/escape;
stronger requirements need an appropriate sandbox/job/container backend or an
explicit unsupported-mode refusal.

Cancellation must stop descendants, drain/close streams, reap what can be
reaped and report cleanup evidence. Signal errors cannot be discarded on a
path that declares cleanup complete. No stale PID/group reuse signalling;
platform-specific identity/containment guarantees must be described and tested.
Do not silently replace a Unix process-tree guarantee with direct-child-only
behaviour on another OS. A backend may report a weaker explicit mode, but not
under the same stronger assurance label.

**LC-11.** Output is part of orchestration. Drain stdout and stderr concurrently
with bounded buffers; bound stdin, framing and total retained output. `piped()`
with no managed reader must be impossible on the supported facade. Choose an
explicit overflow policy: backpressure, fail, or truncate with a marked
incomplete artifact. Never parse truncated JSON as a valid script result.
Separate execution status, output availability and postcondition verification.

## Durability without fiction

**LC-12.** Local execution may retain ordinary borrowed Rust state. Durable
execution MUST use either a restartable registered body whose external calls
are recorded, or a versioned compiled program using those same operation
boundaries. Neither representation serializes an arbitrary Future or stack.
Replay validates step identity, operation ID, input digest and definition
revision before reusing a recorded output. Divergence is a typed stop, not
permission to start new external work.

An opaque script is one coarse effect unless it declares a checkpoint protocol.
A process restart does not resume an arbitrary instruction pointer. Don't
advertise durable step recovery by wrapping a shell script that has already
made unrecorded network mutations. Put those effects behind the host adapter,
or mark the coarse effect Unknown and use its documented reconciler.

**LC-13.** Failover fences dispatch owners, preserves logical idempotency and
blocks uncertain in-flight effects until reconciled. Leases require a
store/authority-verified owner epoch; TTL expiry alone cannot prove an old
worker stopped. Do not garbage-collect receipts, deduplication keys or pinned
artifacts before the declared replay/reconciliation horizon. Compaction records
supersession and preserves unsettled effects and still-referenced evidence.

## Rust implementation requirements

**LC-14.** Keep native async at static call sites; boxing belongs at an actual
heterogeneous boundary. Prefer typed IDs, validated private constructors,
associated output types and exhaustive internal state transitions over flags
that admit inconsistent combinations. `#[non_exhaustive]` public enums are
versioning tools, not substitutes for a specified wire protocol. Wire unknowns
must produce typed compatibility failures.

Do not claim `Evaluate` is pure because it takes `&self`: a closure can access
interior mutable or external state. Replay-safe evaluation must be restricted
to a validated expression representation or an explicitly trusted replay
contract. No async suspension while holding a mutable world/resource borrow
needed by sibling work. Local and parallel adapters must both have real tests.

**LC-15.** Cache validated metadata, normalize capability sets once where
possible, reuse bounded scratch, and avoid per-quiet-tick allocation. Preserve
cache/value atomicity while optimizing. A wake-aware local join should avoid
repolling unrelated pending children, but make the change only with recorded
fairness/cancellation tests. Prefer measurement over claims about zero-cost
abstraction, ECS or hand-written schedulers.

**LC-16.** A small explicit state module, dispatch module, resource-owner module
and report module may remain inside the existing crate; no new top-level crate
or dependency is required by this design. Split along invariants/ownership, not
arbitrary line-count targets. Do not lower existing lint, feature or test gates
to make the facade look simpler.
