# Frontier definition

Continued in: [Benchmarks](frontier-benchmarks.md), [Task mining](frontier-task-mining.md), [Workflow verification](frontier-workflow-verification.md), [Capability authority](frontier-capability-authority.md), [Mixed initiative](frontier-mixed-initiative.md), [Tamper-evident logs](frontier-tamper-evident-logs.md), [Uncertainty and abstention](frontier-uncertainty-abstention.md), [The synthesis](frontier-synthesis.md).

Status: reference. What the state of the art is, and what it decides for
`lgwks_bot`. Companion to [`guidance-runner-spec.md`](guidance-runner-spec.md) and
[`estate-asset-inventory.md`](estate-asset-inventory.md).

## Why this exists

The bot's target is not "better than WalkMe". It is the best bot that can be
built. That slogan alone cannot be falsified, so this page gives a definition. It
names nine areas where the frontier can be located. For each, it says what is
settled, what is open, and the decision it forces on the design: the only part
that matters.

The method: nine parallel literature sweeps over a live search index, the OpenAlex
citation graph and paper text, anchored on measured results, not claims. Every
number below is from a fetched leaderboard, paper or patent record. Where a
source was unreachable, the page states the limit.

Two conventions. **Solved** means no longer a research problem: build against it
and spend nothing re-deriving it. **Open** means the field does not know, so a
decision here is a bet. Record it as one.

Provenance, re-checked 2026-10-02. The Anchors below are **research**: arXiv
papers, patents, leaderboards, RFCs and standards, cited by title, and assessed
as research, not measured on this crate. The few shipping projects named were
re-pinned against their own releases at that date (OpenAdapt launcher `v1.16.0`,
Flow `1.34.0`). A claim the named project does not support at that version is
marked *unsourced*, not removed.

---

## 1. Element grounding: the sketch is half right, and the wrong half carries the weight

**Frontier claim.** Nothing replaced hand-weighted multi-vector scoring on the
DOM. It is still the recall engine, with a learned model only as a top-N
reranker. On pixels-only grounding, learned models won outright. No confidence
threshold can separate "the element moved" from "the element is gone": the
field's answer is a typed outcome, not a score.

**Anchors.**

- Similo (ACM TOSEM 2022): weighted multi-attribute similarity over 40 real
  popular sites, 598 cases. 72 failures against the XPath baseline's 146, i.e.
  **88.0% vs 75.6%**. Hand-weighted scoring on real drifted pages is measured.
- Similo LLM (arXiv 2310.02046): 804 element pairs, 48 real applications,
  failures 70 → 39. The LLM is a **reranker over the weighted scorer's top-N**,
  not a replacement.
- ScreenSpot-Pro leaderboard (2026): pixels-only went **18.9% → 82.7%** in about
  eighteen months. On pixels, learned won outright.
- Set-of-Mark (MSR 2023): the settled interface. The model emits a *mark ID*, and
  the DOM resolves it to an exact node. Selection is learned; identity is
  structural. No coordinate is ever emitted.
- False-heal study (Aug 2026, 133 scenarios, 4 multi-provider runs): on
  **removed** elements, the heuristic baseline false-heals 40.5–57.1% of the time.
  Deleted-element decoys score **[0.665, 0.955]**; true compound drift scores
  **[0.749, 0.874]**. The ranges overlap, so no threshold, margin or filter
  separates them. LLM consensus was right 52/52 on surviving elements and
  **wrong 34/34 on deleted ones**. It declined once in 78.
- Patents: WalkMe **US10713068** and **US11720379** (priority 2019-09-19, expiry
  2039-09-19) and UiPath **US11200073B1** (expiry 2040-11-20, with EP/CN/JP/KR
  family members). Exactly two assignees fence multi-vector element recognition.

**Solved.** Re-resolving a *moved* element with DOM or accessibility-tree access.
Multi-attribute weighted similarity plus a top-N semantic reranker reaches ~95% on
published real-page element-pair benchmarks. Pixel grounding of a *visible*
element is solved. Set-of-Mark is the settled learned/structural interface.

**Open.** *Moved* against *gone* is **provably unsolved by structural evidence**.
The distributions overlap, so absence detection is bounded by the math, not just
untuned. No calibrated confidence means anything here.

**What it changes.** Three decisions. The first is the most important thing on
this page.

1. **The resolver returns a typed three-way, and `ElementRef` is minted only from
   the resolved case**: `Resolved(ElementRef)` / `Ambiguous(Vec<Candidate>)` /
   `Absent`. Moved against gone cannot be a scalar threshold, so a healed or
   guessed element has **no type-level path into `Execute`**. This is the
   proof-carrying pattern the crate uses for authority, applied to observation.
   It is the most defensible idea this project has.
2. **Recognition is a cascade, not a flat sum.** The hand-weighted multi-vector
   scorer is the deterministic default path: millisecond-scale, ~88% recall, zero
   tokens. A learned reranker is consulted only over the top-N when the score
   margin is low. Never a model on the default path.
3. **If a model enters the loop, it returns a mark ID the resolver minted, never a
   coordinate.**

One commercial flag: **get claim charts for the WalkMe 2019 family and UiPath
US11200073 before shipping a multi-vector scorer.** The architecture above is the
mitigation: the patent exposure is in the weighted-score internals, so
Set-of-Mark binding is the independently derived path.
