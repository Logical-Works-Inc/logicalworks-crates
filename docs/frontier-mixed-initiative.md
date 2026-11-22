# Frontier definition: Mixed initiative

Part of [Frontier definition](frontier.md).

## 7. Mixed-initiative and handoff: three criteria replaced the initiative question

**Frontier claim.** Three decision-theoretic criteria replaced "who should act
next?", and each can be implemented directly. Ask when the expected value of the
answer exceeds the cost of asking. Abstain when the top posterior falls below a
rejection threshold. Hand off when the human's expected cost beats the system's.

**Anchors.**

- Horvitz (CHI 1999): the classical framing. Design for uncertainty in both the
  user's intent and the system's ability. Descriptive, not computable.
- Amershi et al., *Guidelines for Human-AI Interaction* (CHI 2019): 18
  evidence-based guidelines indexed by *when* they apply. **G10 "Scope services
  when in doubt"** and **G9 "Support efficient correction"** make scoping-out and
  redirection design primitives, not error states.
- Chow (IEEE Trans. Inf. Theory 1970), extended by Mozannar & Sontag (NeurIPS
  2020): the abstention and **learning-to-defer** criterion, made trainable and
  consistent.
- Rao & Daumé III (ACL 2018): clarification-question ranking by **expected value
  of perfect information**.
- Bohus & Rudnicky (2008) and the spoken-dialogue error-recovery line: a
  non-understanding re-prompt must **escalate in form** while the attempt count
  stays bounded.
- XSTest (2024) and OR-Bench (2024): refusal is now benchmarked on **both** axes,
  under-refusal *and* over-refusal. Refusal has a measured false-positive rate.

**Solved.** The handoff criterion exists and is named. Re-asking on
non-understanding is the settled finding, not a local heuristic, and the re-ask
must escalate in form. Refusal is a studied terminal outcome with two-sided
quality criteria. The case where the human drives the UI is the dominant framing,
not a variant.

**Open.** Calibrated deferral under *real* expert cost, which is unknown before
the human is in the loop. Over-refusal has benchmarks but no principled operating
point. No accepted criterion says *which* repair form to escalate to on the n-th
non-understanding. Nothing composes value-of-information, deferral and rejection
into one decision rule over {act, ask, refuse, hand off}.

**What it changes.**

- **The resolver returns a posterior plus a margin**, and the runner applies a
  three-way rule. Above the accept threshold, advance. Ambiguous: **re-ask the
  same node**. Below the abstain threshold, terminate.
- **`Refused` is reachable from a scope predicate evaluated before resolution**,
  and `HandedOff` from a per-node declared human-cost comparison. Neither is a
  catch-all for the step-budget branch.
- **The re-ask counter is per node, bounded, and escalates in form**: verbatim
  repeat, then narrowed options or explicit confirmation of the top hypothesis,
  then terminal. Recovery falls off after about the second repair, and an
  unbounded re-ask is the hostile loop the design already warns about.
- **A two-sided refusal metric belongs in the session evidence**: the false-refuse
  rate beside the false-complete rate. Over-refusal is a measured defect, and
  proof-carrying authority has no other way to detect it.
- **The session exposes the current node and the reason for the last
  transition.** Without them, the design reproduces the mode-confusion failure.
