# Frontier definition: Uncertainty and abstention

Part of [Frontier definition](frontier.md).

## 9. Uncertainty and abstention — the four-valued verdict is not over-engineering, and abstention is not a free move

**Frontier claim.** "Can we get a confidence number" is solved — calibration and
conformal prediction both work and ship. The frontier moved to two harder places:
keeping the guarantee valid under the drift the system experiences, and
the newly-measured result that **abstention reshapes the human's error rather than
restoring their baseline**.

**Anchors.**

- **Bauer, Leucker & Schallhart** (J. Logic and Computation 2010) — defines a
  four-valued runtime-verification logic whose values are *satisfied / violated /
  presumably violated / presumably satisfied*, refining LTL₃'s three-valued
  good-prefix / bad-prefix / inconclusive. This is the direct formal antecedent for
  a four-valued verdict, and it is derived from axioms, not invented. Their earlier
  **monitorability** result is the load-bearing one: `G(r → F a)`-shaped properties
  have **no good or bad finite prefix**, so they are permanently inconclusive —
  some constraints cannot be decided from a finite observation, ever.

- **Belnap** (Artificial Intelligence 1998) — the other antecedent: truth and
  falsity as *independent* evidence, giving {true, false, both, neither}. That is
  the right algebra when corroborating and refuting evidence arrive from separate
  detectors, which is exactly the DOM-heuristic case.

- Guo et al. (ICML 2017) and *Revisiting the Calibration of Modern Neural Networks*
  (NeurIPS 2021) — modern nets are systematically overconfident; temperature
  scaling fixes it in-distribution and **does not hold out-of-distribution**. Raw
  model confidence is not a usable abstention signal.
- Angelopoulos & Bates (2023) and **Gibbs & Candès** (Annals of Statistics 2023) —
  conformal gives distribution-free finite-sample coverage; adaptive conformal
  restores a **long-run** coverage guarantee under arbitrary drift, with
  step-to-step coverage fluctuating. It does not guarantee the *next* decision.

- **Jabbour et al.** (arXiv 2508.07617, N=259 clinicians) — the decisive abstention
  study. Accuracy was 66% with no AI, 56% with inaccurate AI and 64% with selective prediction. But
  abstention **shifted the error distribution**: +18% missed diagnoses and +35%
  missed treatments versus *no AI at all*. The assumption that "when the AI
  abstains, the human decides as if no AI had spoken" is **empirically false**.

- Measured failure taxonomies, converging — CUADebug (arXiv 2608.02643: 204 failed
  OSWorld trajectories, human-annotated root cause; structured re-execution lifts
  recovery **13.9% → 29.9%**) and *Why Do LLM-based Web Agents Fail?* (ACL 2026).
  Across gpt-5-nano, claude-haiku-4.5 and gemini-flash-2.5, **low-level execution —
  perceptual grounding and control — is the dominant bottleneck**, not high-level
  planning.

**Solved.** Confidence *can* be made honest. The cost question is answered:
conformal's marginal runtime cost is one nonconformity score plus a quantile
lookup, negligible beside a model call — **the real cost is data, not compute**,
because you need an exchangeable labelled calibration set *of the exact decision
you are gating*. The failure taxonomy is measured, not theorised.

**Open.** There is **no published A/B deployment** showing conformal-gated
abstention improves end-to-end reliability of an agent acting on a live UI — which
is precisely why the "thousands of lines of DOM heuristics that pass fixtures and
miss the real element" failure mode remains undefended. Aleatoric-versus-epistemic
is theoretically clean and practically unstable; do not build on the label, build
on computable proxies. **Coverage is not correctness** — if the prediction set is
large the coverage guarantee is worthless for acting, so **set size, not coverage,
is the operational quantity**, and controlling set size under drift is open. And
no head-to-head study compares a four-valued run verdict against two-valued
pass/fail on a long-horizon benchmark: the formalism is well-founded, the empirical
claim that it catches more real failures is untested.

**What it changes.**

- **The four-valued verdict stands, and it is not over-engineering.** Verified /
  failed / unverified / fatal map onto good prefix / bad prefix / inconclusive /
  monitor-abort, and the two "presumably" cases are the two sides of the
  uncertainty an unverified verdict must carry. Grounded in RV-LTL and Belnap, not
  invented here.
- **Enforce the run rule as a lattice meet over step verdicts** (Kleene
  conjunction) rather than a special case, so partial unverification propagates to
  the run verdict by construction.
- **The invariant DSL must reject non-monitorable constraints at declaration
  time.** A `G(r → F a)`-shaped property has no good or bad finite prefix and can
  only ever be unverified; either forbid it or mark it permanently inconclusive.
  This is a *load-time* check, which is exactly what the register's refusal
  machinery is for.
- **Escalation must not hand the task back as if the bot were absent.** Jabbour et
  al. shows that path produces a measurably worse error profile, so the escalation
  payload carries the partial evidence and the named unverified constraint. This
  changes `HandedOff` from a bare target into a payload.
- **Use one nonconformity score for three jobs** — abstention, drift detection, and
  deciding to re-observe — and apply conformal risk control at the **trajectory**
  level, reserving per-click conformal for replayed traces where a per-click
  calibration set exists.
- **This closes the loop with section 4.** The measured dominant failure is
  grounding and control, not knowledge — so an uncertainty model that only
  represents what the bot *knows* misses most of the failure mass. The uncertainty
  has to be about **acquisition**, which is the three-way resolver verdict from
  section 1.
