# Guidance runner specification: Brittleness sweep

Part of [Guidance runner specification](guidance-runner-spec.md).

## Corrections from the brittleness sweep

A second sweep asked what makes this class of system fragile (sequence-dependent
defects, silent drift, effects retried into duplicates) and what the measured
answer is. These bind the three workstreams the same way the frontier corrections
above do. Each names the module it lands on.

**1. `Execute` cannot say "outcome unknown", and that is a correctness defect, not
a missing convenience.** The `BotError` enum carries exactly six variants
(`CapabilityDenied`, `IncompleteSpec`, `SpecTooLarge`, `MalformedSpec`,
`DomainError`, `EvaluateError`). None distinguishes *the effect did not happen*
from *the effect may have happened and the response was lost*. Exactly-once
delivery is impossible and at-least-once is the floor, so a timed-out effect typed
as `Failed` is retried straight into a duplicate. Required:

- An `Indeterminate { intent_id, observed_at }` state.
- A durable intent record written *before* the effect.
- A deterministic key derived from `(flow instance, step, content hash)`.
- Resolution only by a later read-back.

Where the target offers no read-back, the step declares at-most-once and carries a
compensating step, not an automatic retry. No standard helps here: the IETF
`Idempotency-Key` draft **expired**, so this is the vendor's contract to define,
not one to assume.

**2. Soundness is decidable only while the flow language stays inside a bound.**
Soundness is EXPSPACE-complete in general, polynomial for free-choice nets, and
**undecidable** once reset or cancellation arcs are added. So the flow language
refuses cancellation-style constructs and non-free-choice shapes outright, and
the spec's byte cap (`MAX_SPEC_BYTES` / `SpecTooLarge`) gains a semantic sibling.
Expressiveness is given up on purpose, for a checker that terminates.

**3. The soundness triple is safety, and cannot express liveness.** All three
properties are safety properties: a violation has a finite witness, so bounded
exploration can refute it. AWS's retrospective on a decade of TLA+ across ten
systems records the counter-example: *"Failed to find a liveness bug as we did
not check liveness."* A checker proves only what was specified. So each flow
**declares its liveness property explicitly** (every accepted step eventually
reaches a terminal). A flow that declares none is refused at load, like a missing
capability in `GrantSet::admit`.

**4. Flakiness is a typed outcome with a quarantine, never a silent retry.**
Measured: Google sees 1.5% of test runs and roughly 16% of 4.2M individual tests
flaky, and spends **2–16% of its testing budget** rerunning them. Microsoft
measured 4.6% of test cases flaky, **86% reproducible only in CI**, and flaky
tests as the second of ten reasons deploys are slow.

The gain is in *when*: **75% of flaky tests are flaky in the commit that
introduces them**, and detecting on new or modified tests catches 85%. So a
bounded re-run applies to newly authored or modified steps only, not every
execution. An observation that differs across identical replays gives a typed
`Flaky` outcome and a **quarantined** flow. Retries do not fix flakiness: that
budget figure is the cost of masking it. A silent retry turns a diagnosable drift
into an invisible one, the failure this design exists against.

**5. The spec is an external contract and must be additive-only.** Measured:
**13.9% of releases and 11.7% of packages** break on *non-major* dependency
updates. **44%** of manifesting breaks ship in minor or patch releases. Recovery
takes **~134 days** when the provider does not fix it, against a **7-day** median
when they do. So:

- Every public enum is `#[non_exhaustive]`.
- `from_json` rejects unknown fields **loudly**. A silently dropped field is the
  drift mechanism behind that 13.9%.
- `cargo-semver-checks` runs in CI, so a break is a build failure, not a 134-day
  recovery.
- No speculative `SpecV2` beside `BotSpec`: that is two implementations of one job.

**6. Steps must be drivable from a harness, and that is a decision at the trait.**
Stateful and model-based property testing is the one technique with a measured
record on sequence- and interaction-dependent defects: the class a session
runner's bugs fall in. The canonical demonstration is a hand-verified queue that
fails after five generated tests, with a shrinkable trace. The technique is out
of reach if a step runs only through a live UI. So the effect seam splits a
`Model` from the system under test at the step trait. A step that cannot be
driven deterministically is a defect **at the trait**, not an inconvenience at
the test.

**Rejected: consumer-driven contract testing as machinery.** Its whole empirical
base is four repositories. No published false-positive or false-negative rate
exists in either direction. Its structural N+M adoption tax is worst here, where
there is one consuming boundary. Take the *check*, and leave out the DSL, the
broker and provider-side verification. A step declares the read-subset of the
foreign surface it consumes. Spec load refuses it if that subset is outside the
provider's declared shape, beside the `SpecTooLarge` / `MalformedSpec` refusals.
Record the fidelity limit in the type: a subset check proves **shape**
compatibility, never behaviour, so it must not issue a `Grant`.
