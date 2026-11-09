# Guidance runner specification

Continued in: [Workstream A](guidance-runner-spec-workstream-a.md), [Workstream C](guidance-runner-spec-workstream-c.md), [Frontier sweep](guidance-runner-spec-frontier-sweep.md), [Brittleness sweep](guidance-runner-spec-brittleness-sweep.md), [Element resolution](guidance-runner-spec-element-resolution.md).

Status: **landed, kept as the design record.** All three workstreams shipped:
`lgwks_std::similarity`, `lgwks_bot::session` and `contract/INVARIANTS.toml`. The
corrections below are what the sweeps asked of them, and a correction with no code
named in this tree is still a target: no `FlowSource` seam, no typed `Flaky`
outcome and no `cargo-semver-checks` lane exist. `BotError::EffectIndeterminate`
answers brittleness correction 1.

## Purpose

`lgwks_bot` executes four verbs (Observe, Evaluate, Execute, Query) on a
change-detecting ECS schedule with proof-carrying authority. It is a substrate.
It has no session, no dialogue state machine, no variable scope, and no
declarative flow document, so it cannot carry the class of workload a digital
adoption platform carries: a deterministic, authored, human-facing conversation
with recorded outcomes.

This specification adds that layer. It does not change the four verbs, the
authority model, or the schedule.

## What already exists in the workspace, and what is deferred

The layer described here is not novel in this organization. A canonical flow
representation already exists elsewhere and is the reference design.

| Asset | Location | Status |
|---|---|---|
| `FlowSpec`, `FlowNodeKind`, `FlowEdge`, `Disposition`, `ChoiceArm`, `Predicate`, `ValueExpr`, `FlowBounds`, `JustificationDecl` | `sentinel/braid/crates/braid-flow-ir` | **Source may be copied.** Braid is deprecated (confirmed 2026-09-20) and the crate is MIT, owned by this organization. Copy where the code is self-contained; port where it is not |
| `ValueExpr` / `Predicate`, `src/predicate.rs` | same | Copy the design. It is a closed, side-effect-free expression language (const, Eq, Ne, Lt, Le, Gt, Ge, And, Or, Not) over a scalar. It needs `braid_ir::Value`, which this workspace does not have, so substitute this workspace's own scalar type. This is the branch-condition language Workstream B needs and it is 133 lines |
| `src/preflight.rs` | same | **Do not copy.** It is wire-byte hardening against untrusted canonical input (2 MiB-plus envelopes, nesting depth, container counts). It has no role here: this workspace parses its own documents through `lgwks_std::json`, not through an adversarial canonical decoder |
| `src/decode.rs`, `encode.rs`, `wire_depth.rs`, `literal.rs` | same | **Do not copy.** Canonical content-addressed wire encoding. It exists to give Braid flows a stable `Cid`, and it drags `braid-ir`'s `Cid`, `TypeTag` and `Value` with it. Nothing in this stack needs content-addressed flows |
| Declarative plan compilation with interpolation and lane resolution | `keel/src/construct/` | Reference design, same constraint |
| Sealed evidence snapshots (`toolchain`, `git`, `host`, `env_fingerprint`, `runner`) | `keel/src/evidence/audit_log/` | Reference design, same constraint |
| `Session`, `SessionId`, `SessionInvariant` | `secure-authority/crates/logicalworks-auth/src/session.rs` | Reference design for the session identity shape |

The consequence for this task: **port the shape, not the dependency.** Every
ported concept sits behind a trait so that when `braid-flow-ir` is published, or
when these modules are lifted into this workspace, each is replaced without
touching a call site. That is the design requirement the rest of this page
serves.

This table is not the whole picture. A full sweep of adjacent repositories is in
[`estate-asset-inventory.md`](estate-asset-inventory.md), and it found two assets
that are directly relevant: a journal-authoritative session runtime already
exists under a compatible licence, and so does the similarity scoring primitive.

**They are references, not replacements.** An earlier draft of this page
proposed collapsing Workstream B onto the existing runtime. That was rejected by
the maintainer, and the reason governs the whole design:

> The bot is the general instrument. It is not an implementation of one
> workload, and it does not get narrower because a sibling repository happens to
> solve a similar problem for a different purpose. The standard is the best bot
> that can be built, not the smallest one that satisfies today's consumer.

So the estate assets inform the *shape* — a bounded journal, an authoritative
fold, a revocation clock — while the bot keeps its own seams and its own
abstractions. Where a sibling asset is genuinely the right mechanism later, it
arrives as a new `impl` behind a seam that already exists, not by narrowing the
bot's surface. Workstream A still consumes rather than duplicates the similarity
primitive, because that is a leaf calculation with one correct answer; the
distinction is between a leaf that has one answer and a core that must stay
general.

## Design rule: every borrowed part is behind a trait

Nothing below may be used directly by a caller at more than one remove.

| Concept | Seam | Replaced later by |
|---|---|---|
| Similarity metrics | `trait Similarity` | A fitted or learned scorer |
| Free-text intent resolution | `trait Resolver` | A model-backed resolver |
| Flow document encoding | `trait FlowSource` | `braid-flow-ir`'s canonical bytes |
| Session record sink | `trait Journal` | The sealed evidence log |
| Variable interpolation | `trait Interpolate` | `keel::construct::plan_load::interpolate` |

A seam that has exactly one implementation today is still a seam. The point is
that tomorrow's replacement is a new `impl`, not a rewrite.
