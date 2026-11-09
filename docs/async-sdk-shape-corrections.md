# The async surface as an SDK: Corrections

Part of [The async surface as an SDK](async-sdk-shape.md).

## Corrections to the earlier proposal

1. **Scope/spawn is not the front door.** Structured lifetime remains necessary,
   but consumers should name business work rather than manage child handles.
   A result reference is not ownership of the execution; dropping a view must
   not abandon its owner.
2. **No second mandatory `PlanSpec`.** Native task code, existing `BotSpec` and
   existing `FlowSpec` must converge on registered typed operations and one
   execution/recording path. They are not equivalent today: `FlowSpec`'s Route
   is a node transition, not a tool call. Evolve versioned node forms where
   needed instead of asserting that a materializer already exists.
3. **No magical scoped parallel borrowing.** Same-task local composition may
   borrow and remain non-Send. Parallel/remote work crosses an explicit owned
   boundary. Do not promise all lifetime/execution properties simultaneously;
   see the Rust research in the evidence document.
4. **No destructor-as-join claim.** Rust allows safe values to be forgotten and
   Drop cannot await. The host owns cleanup, reports unresolved cleanup and
   provides an explicit drain. Memory safety must not depend on a caller
   polling a future to completion.
5. **No absolute no-spawn claim about `lgwks_std::task`.** It has a
   `spawn_blocking` facility. Scheduling policy must cover actual entry paths,
   not names the guide wishes were absent.
6. **No reopening owner decisions by implication.** The first-party scheduler
   direction and the accepted existing time layers remain recorded decisions.
   Exact simulated replay is a separate, unproved capability; wall-clock APIs
   alone do not establish it. Any additional clock seam requires its own
   implementation and acceptance, not a claim that it has already landed.

## Entry modes and guarantees

`run` is a scoped invocation under a live host. A deliberately persistent
submission is a separately named operation on a configured durable host; it is
not the accidental consequence of dropping a handle. Local borrowed tasks
cannot be submitted as remote/durable work.

An arbitrary async function may run locally. It does not thereby become
serializable or restartable. Durable tasks use versioned registered operation
boundaries and recorded results; opaque scripts are a single effect unless
they opt into an explicit checkpoint protocol. Model calls are external,
potentially costly operations, not pure expressions recomputed during replay.

## Delivery order

Repair the concrete lifecycle/cache defects first or alongside the facade;
then land one typed local task returning a real result, bounded composition,
owned process/readiness operations, registry materialization and the verified
PR-review journey. Durable resumption and AI proposal helpers build on those
same boundaries. A backend without a real consumer/test is not closure.

The [acceptance specification](orchestration-acceptance.spec.md) is the release
contract. Merging these documents alone does not close #87.
