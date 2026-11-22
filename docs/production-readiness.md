# Production readiness of `lgwks_bot`, and why it replaces non-AI automation

Status: **diagnostic, provisional, and a NO-GO for unattended consequential
automation.** This page shows what is measured, what is proved, what is only
argued, and what must be true before you point this at money, messages or
infrastructure without watching it.

Argue with it. Every score below is against a named gate, and several are ❌
against this crate, not a rival. A wrong number or a wrong gate is a defect in
this page: file it.

Companions: [`framework-comparison.md`](framework-comparison.md) for the field
survey, [`frontier.md`](frontier.md) for the nine research areas,
[`effect-kernel.md`](effect-kernel.md) for the durability design,
[`async-parity.md`](async-parity.md) for the runtime surface, and
[`../bench/README.md`](../bench/README.md) for the measurement rig and its raw
numbers.

This page is the index. The sections live under
[`production-readiness/`](production-readiness/headline.md), because a `.md`
file never exceeds 120 lines. Each keeps its number, heading and score here.

---

## 1. The headline, before the method

**This does not pass its own release gate.** Issue
[#109](https://github.com/Logical-Works-Inc/logicalworks-crates/issues/109) held eight
invariants: five were observed externally against a real journal under a real
`SIGKILL`, and three closed on their repair alone. Ship this for attended,
inspectable automation where a person can step in. Do not ship it yet for a bot
that runs for weeks and acts without you.

- [The headline, and the update of 2026-09-28](production-readiness/headline.md)
- Since then: the VPS profile was measured under a 2 CPU / 2 GiB cgroup with the
  closed-loop latency target met ([`bench/async/vps.md`](../bench/async/vps.md)),
  and the acceptance campaign ran 251 of 251 named tests on both containment
  backends in CI run
  [38795282994](https://github.com/Logical-Works-Inc/logicalworks-crates/actions/runs/38795282994).
  The dated update pages are in git history before 2026-10-27.

## 2. The claim under test

`lgwks_bot` replaces non-full-AI automation and RPA: the mechanism half holds, the
product half not yet. [Read §2 and §3](production-readiness/claim-and-rivals.md).

## 3. Why each rival should win

Each rival has a real reason to be chosen. [Read §2 and §3](production-readiness/claim-and-rivals.md).

## 4. Nine axes

The gates are the estate's SLO evidence matrix. Each axis is scored ✅ (measured
or proved at the stated gate), ⚠️ (partly covered, with the missing half named)
or ❌ (not covered at a level that supports a production claim).

### 4.1 Frontier: does the design beat credible alternatives?

**✅** Argued from a survey. On authoring, the facade loses to `futures`.
[Read 4.1](production-readiness/4-1-frontier.md).

### 4.2 Hyperscale: more than a million concurrent, correct

**⚠️** A million concurrent, six knees, a 2 CPU / 2 GiB cgroup run; no real VPS.
[Read 4.2](production-readiness/4-2-hyperscale.md).

### 4.3 Idiomatic: ownership, errors, lifetimes sound

**✅** Compiler-enforced, with one named hole in the proof chain.
[Read 4.3](production-readiness/4-3-idiomatic.md).

### 4.4 Generalized: works across declared inputs, messy ones included

**⚠️** The declared input space is covered; the messy one is not.
[Read 4.4](production-readiness/4-4-generalized.md).

### 4.5 Decoupled: components independently replaceable

**✅** The seams are real and load-bearing; one consumer's evidence.
[Read 4.5](production-readiness/4-5-decoupled.md).

### 4.6 Ephemeral: survives process loss

**⚠️** The design is right; five of #109's eight observations ran on a real
store. [Read 4.6](production-readiness/4-6-ephemeral.md), then
[the ceiling and continuation](production-readiness/4-6-ephemeral-continuation.md).

### 4.7 Portable: same semantics on all declared targets

**⚠️** Linux and macOS run suites, Windows one containment job, seven targets
built; a build receipt is not an execution receipt. [Read 4.7](production-readiness/4-7-portable.md).

### 4.8 Multi-tenant: isolated under concurrent use

**⚠️** Covered at the stated boundaries, with the missing half named.
[Read 4.8](production-readiness/4-8-multi-tenant.md).

### 4.9 Performance: fastest correct implementation

**⚠️** The synchronous tick loses by 26x to 2 738x; the async facade by 2% to
17% at p50. [Read 4.9](production-readiness/4-9-performance.md).

## 5. Why Rust, without the adjectives

What the language gives and what it does not.
[Read §5 and §6](production-readiness/5-6-rust-and-exclusions.md).

## 6. What this evaluation leaves out

Each exclusion is named with its direction of bias.
[Read §5 and §6](production-readiness/5-6-rust-and-exclusions.md).

## 7. The verdict, and the gate that would change it

**Today: not production-ready for unattended consequential automation.** Six items
would change that, in order. [Read §7](production-readiness/7-verdict.md).
