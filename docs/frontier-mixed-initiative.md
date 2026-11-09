# Frontier definition: Mixed initiative

Part of [Frontier definition](frontier.md).

## 7. Mixed-initiative and handoff — the initiative question is retired; three criteria replaced it

**Frontier claim.** "Who should act next?" has been replaced by three
decision-theoretic criteria that are each directly implementable. Ask when the
expected value of the answer exceeds the cost of asking, abstain when the top
posterior falls below a rejection threshold, and hand off when the human's
expected cost beats the system's.

**Anchors.**

- Horvitz (CHI 1999) — the classical framing: design for uncertainty in both the
  user's intent and the system's ability. Descriptive, not computable.
- Amershi et al., *Guidelines for Human-AI Interaction* (CHI 2019) — 18
  evidence-based guidelines indexed by *when* they apply. **G10 "Scope services
  when in doubt"** and **G9 "Support efficient correction"** make scoping-out and
  redirection design primitives rather than error states.
- Chow (IEEE Trans. Inf. Theory 1970), extended by Mozannar & Sontag (NeurIPS
  2020) — the abstention and **learning-to-defer** criterion, made trainable and
  consistent.
- Rao & Daumé III (ACL 2018) — clarification-question ranking by **expected value
  of perfect information**.
- Bohus & Rudnicky (2008) and the spoken-dialogue error-recovery line — a
  non-understanding re-prompt must **escalate in form** while the attempt count
  stays bounded.
- XSTest (2024) and OR-Bench (2024) — refusal is now benchmarked on **both** axes:
  under-refusal *and* over-refusal, i.e. refusal has a measured false-positive rate.

**Solved.** The handoff criterion exists and is named. Re-asking on
non-understanding is the settled finding, not a local heuristic — and the re-ask
must escalate in form. Refusal is a studied terminal outcome with two-sided
quality criteria. The human-drives-the-UI case is the dominant framing, not a
variant.

**Open.** Calibrated deferral under *real* expert cost, unavailable when the human
is not yet in the loop. Over-refusal has benchmarks but no principled operating
point. No accepted criterion for *which* repair form to escalate to on the n-th
non-understanding. And nothing composes value-of-information, deferral and
rejection into one decision rule over {act, ask, refuse, hand off}.

**What it changes.**

- **Make the resolver return a posterior plus a margin**, and let the runner apply
  a three-way rule: above the accept threshold, advance; ambiguous, **re-ask the
  same node**; below the abstain threshold, terminate.
- **`Refused` must be reachable from a scope predicate evaluated before
  resolution**, and `HandedOff` from a per-node declared human-cost comparison —
  so neither is a catch-all for the step-budget branch.
- **The re-ask counter is per-node, bounded, and escalating in form**: verbatim
  repeat, then narrowed options or explicit confirmation of the top hypothesis,
  then terminal. Recovery falls off after roughly the second repair, and an
  unbounded re-ask is the hostile loop the design already warns about.
- **A two-sided refusal metric belongs in the session evidence** — false-refuse
  rate as well as false-complete rate, since over-refusal is a measured frontier
  defect and proof-carrying authority has no other way to detect it.
- **The session must expose the current node and the reason for the last
  transition**, or the mode-confusion failure is reproduced by design.
