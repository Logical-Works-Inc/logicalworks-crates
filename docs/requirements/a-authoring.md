# Requirements A. Authoring

Part of [the product requirements](../../REQUIREMENTS.md), which state how a requirement changes.

## A. Authoring

### R1. A task author writes logic, never lifecycle.

> A developer MUST supply typed work and its logic and receive a result. The
> contract MUST own reaping, readiness polling, retry ownership,
> duplicate-effect suppression, cancellation, continuation persistence, and
> reporting.

**Invariants.** Those seven obligations live below the task surface. None is
reachable as a consumer-written helper that a correct example requires. A
successful function return is not by itself proof of an external state change;
the report separates disposition, output, external-effect knowledge, cleanup
and verification.

**Callers.** A developer authoring a task. An AI authoring a task.

**Falsifier.** Author the reference journey in
`docs/pr-review-orchestration.spec.md` as a task and count the lifecycle lines
below the task body. Any non-zero count refutes R1, including a count hidden in
a helper the example needs for correctness.

**Status.** `partial`, measured. The contract is specified (`DX-01` in
`docs/declarative-orchestration.spec.md`) and the facade ships as `lgwks_bot::task`
and `script!` ([#87](https://github.com/Logical-Works-Inc/logicalworks-crates/issues/87),
closed). Rows T01 and T31 to T34 are exercised and none is accepted, and the falsifier's
lifecycle-line count has not been taken.

### R2. Composition is stringing, not plumbing.

> Combining two or more tasks MUST require no new queue, supervisor, retry
> controller or reporting subsystem, and the composed unit MUST report as one.

**Invariants.** Composition introduces no second lifecycle owner. A partial
failure inside a composition is carried on the composed report, not
accumulated by the caller.

**Callers.** A developer composing tasks. An AI composing tasks.

**Falsifier.** Compose two tasks with `all`, kill one mid-flight. If the call
site needs a join handle, a retry counter, or a partial-failure accumulator to
produce a correct report, R2 is refuted.

**Status.** `partial`, measured. `DX-02` names `all`, `map` and `watch`, and composition
shipped under other names: `Scope::each`, `retry`, `FanOut` and `try_join!`. Row T04
is exercised and not accepted. The named operations remain the target.
