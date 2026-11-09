# Frontier definition

Continued in: [Benchmarks](frontier-benchmarks.md), [Task mining](frontier-task-mining.md), [Workflow verification](frontier-workflow-verification.md), [Capability authority](frontier-capability-authority.md), [Mixed initiative](frontier-mixed-initiative.md), [Tamper-evident logs](frontier-tamper-evident-logs.md), [Uncertainty and abstention](frontier-uncertainty-abstention.md), [The synthesis](frontier-synthesis.md).

Status: reference. What the state of the art is, and what it decides for
`lgwks_bot`. Companion to [`guidance-runner-spec.md`](guidance-runner-spec.md) and
[`estate-asset-inventory.md`](estate-asset-inventory.md).

## Why this exists

The bot's target is not "better than WalkMe". It is the best bot that can be
built. That is an unfalsifiable slogan on its own, so this page replaces it
with a definition. It names nine areas where the frontier is locatable, what is settled in
each, what is genuinely open, and the decision each
one forces on the design, the only part that matters.

The method: nine parallel literature sweeps over a live search index, the OpenAlex
citation graph, and paper text, anchored on measured results rather than claims.
Every number below is from a fetched leaderboard, paper, or patent record. Where a
source was unreachable, the limit is stated rather than the gap papered over.

Two conventions. **Solved** means no longer a research problem — build against it
and do not spend budget re-deriving it. **Open** means the field does not know, so
a decision here is a bet and should be recorded as one.

Provenance, re-checked 2026-10-02. The Anchors below are **research**: arXiv
papers, patents, leaderboards, RFCs and standards, cited by title, and assessed
as research rather than measured on this crate. The few shipping projects named
were re-pinned against their own releases at that date (OpenAdapt launcher
`v1.16.0`, Flow `1.34.0`). A claim the named project does not support at that
version is marked *unsourced* rather than removed.

---

## 1. Element grounding — the sketch is half right, and the half that is wrong is load-bearing

**Frontier claim.** Hand-weighted multi-vector scoring was never replaced on the
DOM; it is still the recall engine, with a learned model used only as a top-N
reranker. On pixels-only grounding, learned models won decisively. And no
confidence threshold anywhere can separate "the element moved" from "the element
is gone" — the frontier's answer is a typed outcome, not a score.

**Anchors.**

- Similo (ACM TOSEM 2022) — weighted multi-attribute similarity over 40 real
  popular sites, 598 cases: 72 failures against the XPath baseline's 146, i.e.
  **88.0% vs 75.6%**. Hand-weighted scoring on real drifted pages is measured.
- Similo LLM (arXiv 2310.02046) — 804 element pairs, 48 real applications:
  failures 70 → 39. The LLM is a **reranker over the weighted scorer's top-N**,
  not a replacement.
- ScreenSpot-Pro leaderboard (2026) — pixels-only went **18.9% → 82.7%** in about
  eighteen months. On pixels, learned won outright.
- Set-of-Mark (MSR 2023) — the settled interface: the model emits a *mark ID*,
  the DOM resolves it to an exact node. Selection is learned; identity is
  structural. No coordinate is ever emitted.
- False-heal study (Aug 2026, 133 scenarios, 4 multi-provider runs) — on
  **removed** elements the heuristic baseline false-heals 40.5–57.1% of the time.
  Deleted-element decoys score **[0.665, 0.955]**; true compound drift scores
  **[0.749, 0.874]**. The ranges overlap, so no threshold, margin, or filter
  separates them. LLM consensus was right 52/52 on surviving elements and
  **wrong 34/34 on deleted ones**. Declining happened once in 78.
- Patents: WalkMe **US10713068** and **US11720379** (priority 2019-09-19, expiry
  2039-09-19) and UiPath **US11200073B1** (expiry 2040-11-20, with EP/CN/JP/KR
  family members). Multi-vector element recognition is fenced by exactly two
  assignees.

**Solved.** Re-resolving a *moved* element given DOM or accessibility-tree access.
Multi-attribute weighted similarity plus a top-N semantic reranker reaches ~95% on
published real-page element-pair benchmarks. Pixel grounding of a *visible* element
is effectively solved. Set-of-Mark is the settled learned/structural interface.

**Open.** *Moved* versus *gone* is **provably unsolved by structural evidence** —
the distributions overlap, so absence detection is mathematically bounded rather
than merely untuned. There is no calibrated confidence that means anything.

**What it changes.** Three decisions, and the first is the most important thing in
this page.

1. **The resolver returns a typed three-way, and `ElementRef` is minted only from
   the resolved case.** `Resolved(ElementRef)` / `Ambiguous(Vec<Candidate>)` /
   `Absent`. Because the moved/gone distinction cannot be a scalar threshold, a
   healed or guessed element must have **no type-level path into `Execute`**. This
   is the proof-carrying pattern the crate already uses for authority, applied to
   observation — and it is the single most defensible idea available to this
   project.
2. **Recognition is a cascade, not a flat sum.** The hand-weighted multi-vector
   scorer is the deterministic default path — millisecond-scale, ~88% recall, zero
   tokens — and a learned reranker is consulted only over the top-N when the score
   margin is low. Never a model on the default path.
3. **If a model enters the loop, it returns a mark ID the resolver minted, never a
   coordinate.**

And one commercial flag: **obtain claim charts for the WalkMe 2019 family and
UiPath US11200073 before shipping a multi-vector scorer.** The architecture above
is the mitigation — the patent exposure is in the weighted-score internals, so
Set-of-Mark binding is the independently-derived path.
