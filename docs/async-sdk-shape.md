# The async surface as an SDK

Continued in: [Corrections](async-sdk-shape-corrections.md).

Status: **design contract; step 1 of the front door has since shipped.** Reviewed
against `51897f8c0cda627d3b3abcee28bda6ebb690f7a1`, 2026-09-21.
Its tracker was [#87](https://github.com/Logical-Works-Inc/logicalworks-crates/issues/87).
This revision replaces the earlier scope-first proposal.

**What has changed since this page was written, so it is not read as the
current SDK.** The `lgwks_bot::task` module now ships `Host`, `Task` and `Report`.
Build a `Host` once with a tenant, stop token, admission ceiling, deadline and trail
capacity, define a `Task` with `task(name, body)`, and call `host.run(&task,
input).await`. The `Report` carries the disposition, the typed output, the
located error and a bounded step trail (INV-BOT-20, exercised by
`tests/it/task_front_door.rs` and `tests/it/sim_task.rs`). **Bodies are polled on the
calling task, so they need not be `Send` and inputs may borrow** — which is the
property this page argued for and which `async-parity.md` §3 now records as
the shipped route for a non-`Send` future.

So this page is **half historical**. The contract and the reasoning below are
still the design of record, and #87 itself is closed, but:

- The symbols below are **not** all proposals any more; the ones in
  `lgwks_bot::task` ship, with the names that module uses.
- **Do not read a symbol spelling below as the available API.** Where this page
  and `lgwks_bot::task` disagree, `lgwks_bot::task` is what a consumer compiles
  against. A proposed spelling that never shipped is a design record, not an
  API.
- `Bot::from_spec` also now materializes a validated `BotSpec` through the same
  registry and assembly path a native bot uses
  (`tests/it/spec_materialize.rs`), so the "no second interpreter" clause below is
  implemented rather than merely required.

Everything after this paragraph is the design as written on 2026-09-21 and is
retained for the reasoning, not as an inventory of what exists.

## The contract

A task author supplies **what to do and the substantive script**, not the
machinery that keeps it running. Reviewing a PR must not require another
application-specific queue, supervisor, tick loop, retry controller, child
process manager, capability-repair loop or reporting subsystem.

Illustrative target, not a compilable example of a shipped API:

```text
let review = task("review-pr", review_script);
let report = host.run(review, pr).await?;
```
A host is configured once with explicit policy and adapters. That configuration
is real work and must be included in usability measurements. A tiny call site
that hides hundreds of bespoke orchestration lines in `review_script` fails
this contract just as surely as a verbose call site does.

The task-first specification consists of:

| Document | Authority |
|---|---|
| [Declarative orchestration](declarative-orchestration.spec.md) | Author-facing semantics, composition, ownership boundaries and migration |
| [Lifecycle](orchestration-lifecycle.spec.md) | Scheduling, attempts, cancellation, blocked work, durability and resources |
| [AI orchestration](ai-orchestration.spec.md) | AI as API consumer and workload, context/evidence, bounded proposals and evaluation |
| [PR review journey](pr-review-orchestration.spec.md) | The required end-to-end task plus script and its external-system boundaries |
| [Acceptance](orchestration-acceptance.spec.md) | Falsifiers, developer-experience criteria and evidence required for release |
| [Evidence](orchestration-evidence.md) | Pinned code findings and primary research; limits of this review |

The specifications prescribe future behaviour. Actual source and exact-version
release notes remain authoritative about what runs today. Where this contract
conflicts with an older design sketch, it governs the proposed task-first
surface, not a silent change to existing runtime behaviour.

## Three audiences, one engine

The **task author** uses typed inputs/results and ordinary async/await. No
`Pin`, engine `JoinError`, worker count, task-plumbing `Arc`, semaphore,
`JoinSet`, detach, readiness sleep or PID cleanup belongs in the common task.
That is an ergonomic acceptance criterion, not a prohibition on domain logic
that genuinely uses an `Arc` or an advanced integrator choosing a lower level.

The **host integrator** installs policy, storage, authority, sandbox and domain
adapters, and chooses the host lifetime once. There is no implicit global
runtime or automatic authority escalation. An existing async host drives work;
a synchronous application explicitly owns runtime entry once.

The **adapter author** implements the real external contracts: typed I/O,
authority requirements, dispatch certainty, readiness, bounded observations,
reconciliation and cleanup. The SDK cannot derive those facts from an opaque
Rust closure. Domain adapters continue to implement Observe, Evaluate, Execute
and Query; composition and supervision are not a fifth domain verb.

## Existing mechanisms to retain

The current typed observation builder is valuable: it relates source output,
condition input and action input before erasure. Keep it. Keep the retained
payload binding and the distinction between refused, not-delivered and
unsettled effects. Keep bounded admission and explicit snapshot authority.
See the [source evidence](orchestration-evidence.md#repository-evidence).

`Runtime`, `Supervisor`, the ECS schedule and the local future driver are
implementation/integration facilities beneath the task-first facade, not a
checklist every application repeats. This proposal does not authorize replacing
Tokio, removing ECS, adding a parallel workflow engine, or adding dependencies
outside `AGENTS.md`'s ownership policy.
