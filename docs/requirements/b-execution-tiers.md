# Requirements B. Execution tiers

Part of [the product requirements](../../REQUIREMENTS.md), which state how a requirement changes.

## B. Execution tiers

### R3. In-process is the default tier.

> Ordinary work MUST run in the calling process with no VM and no runtime
> constructed for it.

**Invariants.** An observe to evaluate to execute path that declares no
cardinality `N` and opts into no compute lane constructs no runtime, spawns no
isolate, and starts no thread.

**Callers.** Every task that is not escalated.

**Falsifier.** A single-source, single-condition, single-effect task that
constructs a runtime to run. Measure construction, not use.

**Status.** `partial`, measured. The synchronous `Bot::tick` path is what
`bench/` measures and it is in-process. Whether a default-feature build
constructs a runtime before the first escalated unit is **unmeasured**.

### R4. Escalation is declared, never inferred.

> A VM or runtime MUST be invoked only when the developer declares cardinality
> `N` for parallel work, or opts the work onto a blocking compute lane. A
> workload MUST NOT escalate on a heuristic.

**Invariants.** The bound is stated at construction. Fan-out is capped at the
declared `N`. The compute lane is opt-in and its use is reported. A workload
that needs more than it declared is refused rather than grown.

**Callers.** A developer declaring `N`. A developer declaring a compute lane.

**Falsifier.** A workload whose parallelism is discovered at run time rather
than declared, or a runtime constructed with no `N` and no compute opt-in.

**Status.** `partial`, argued. `join_all_bounded` and `Supervisor`'s in-flight
ceiling carry the shape; "stated at construction and refused beyond" is the
rule and is not uniformly enforced across the surface.

### R5. Escalation does not transfer ownership.

> Moving work to a runtime or a compute lane MUST NOT hand the caller a
> droppable handle. The escalated unit MUST return a result and a report, and
> the contract MUST still own the bounds, the cancellation and the record.

**Invariants.** No API on the task surface returns a `JoinHandle` or any handle
whose `Drop` starts work or abandons work. A `!Send` future is driven, never
spawned.

**Callers.** Every escalated unit.

**Falsifier.** A call site that stores a handle to keep work alive, or
to abandon it.

**Status.** `met` for the current surface, argued and partially measured. The
crate's thesis is that no verb starts work and hands back a handle to it;
`spawn_local` was withdrawn for exactly this reason, and
`tests/it/rt_async_tier.rs::a_non_send_future_is_driven_rather_than_spawned` is
the discriminating case.
