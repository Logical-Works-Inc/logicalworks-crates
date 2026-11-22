# Frontier definition: Uncertainty and abstention

Part of [Frontier definition](frontier.md).

## 9. Uncertainty and abstention: the four-valued verdict is needed, and abstention is not free

**Frontier claim.** A confidence number is a solved problem: calibration and
conformal prediction both work and ship. The field moved to two harder places.
One is keeping the guarantee valid under the drift the system sees. The other is
a new measurement: **abstention reshapes the human's error and does not restore
their baseline**.

**Anchors.**

- **Bauer, Leucker & Schallhart** (J. Logic and Computation 2010) define a
  four-valued runtime-verification logic: *satisfied / violated / presumably
  violated / presumably satisfied*. It refines LTL₃'s three values (good prefix,
  bad prefix, inconclusive). This is the formal antecedent of a four-valued
  verdict, derived from axioms. Their earlier **monitorability** result carries
  the weight: `G(r → F a)`-shaped properties have **no good or bad finite
  prefix**, so they stay inconclusive. No finite observation decides them.

- **Belnap** (Artificial Intelligence 1998) is the other antecedent: truth and
  falsity as *independent* evidence, giving {true, false, both, neither}. That is
  the right algebra when separate detectors bring corroborating and refuting
  evidence, which is the DOM-heuristic case.

- Guo et al. (ICML 2017) and *Revisiting the Calibration of Modern Neural Networks*
  (NeurIPS 2021): modern nets are overconfident. Temperature scaling fixes it
  in-distribution and **does not hold out-of-distribution**. Raw model confidence
  is not a usable abstention signal.
- Angelopoulos & Bates (2023) and **Gibbs & Candès** (Annals of Statistics 2023):
  conformal gives distribution-free finite-sample coverage. Adaptive conformal
  restores a **long-run** coverage guarantee under arbitrary drift, while
  step-to-step coverage moves. It does not guarantee the *next* decision.

- **Jabbour et al.** (arXiv 2508.07617, N=259 clinicians) is the decisive
  abstention study. Accuracy was 66% with no AI, 56% with inaccurate AI and 64%
  with selective prediction. But abstention **shifted the error distribution**:
  +18% missed diagnoses and +35% missed treatments against *no AI at all*. "When
  the AI abstains, the human decides as if no AI had spoken" is **empirically
  false**.

- Measured failure taxonomies agree. CUADebug (arXiv 2608.02643) annotated the
  root cause of 204 failed OSWorld trajectories, and structured re-execution lifts
  recovery **13.9% → 29.9%**. *Why Do LLM-based Web Agents Fail?* (ACL 2026)
  tested gpt-5-nano, claude-haiku-4.5 and gemini-flash-2.5. **Low-level execution
  (perceptual grounding and control) is the dominant bottleneck**, not high-level
  planning.

**Solved.** Confidence *can* be made honest, and its cost is known. Conformal's
runtime cost is one nonconformity score plus a quantile lookup, small beside a
model call. **The real cost is data, not compute**: you need an exchangeable
labelled calibration set *of the exact decision you gate*. The failure taxonomy
is measured, not theorised.

**Open.**

- There is **no published A/B deployment** showing that conformal-gated abstention
  makes an agent on a live UI more reliable end to end. That is why the "thousands
  of lines of DOM heuristics that pass fixtures and miss the real element" failure
  stays undefended.
- Aleatoric against epistemic is clean in theory and unstable in practice. Build
  on computable proxies, not on the label.
- **Coverage is not correctness.** A large prediction set makes the coverage
  guarantee worthless for acting, so **set size is the operational quantity**.
  Controlling set size under drift is open.
- No head-to-head study compares a four-valued run verdict against two-valued
  pass/fail on a long-horizon benchmark. The formalism is well-founded; the claim
  that it catches more real failures is untested.

**What it changes.**

- **The four-valued verdict stands.** Verified / failed / unverified / fatal map
  onto good prefix / bad prefix / inconclusive / monitor-abort. The two
  "presumably" cases are the two sides of the uncertainty an unverified verdict
  carries. RV-LTL and Belnap ground it.
- **Enforce the run rule as a lattice meet over step verdicts** (Kleene
  conjunction), not a special case, so partial unverification reaches the run
  verdict by construction.
- **The invariant DSL rejects non-monitorable constraints at declaration time.** A
  `G(r → F a)`-shaped property has no good or bad finite prefix and can only stay
  unverified: forbid it, or mark it inconclusive for good. This is a *load-time*
  check, which the register's refusal machinery is for.
- **Escalation must not hand the task back as if the bot were absent.** Jabbour et
  al. show that path gives a worse error profile. So the escalation payload carries
  the partial evidence and the named unverified constraint, and `HandedOff` becomes
  a payload, not a bare target.
- **One nonconformity score does three jobs**: abstention, drift detection and the
  choice to re-observe. Apply conformal risk control at the **trajectory** level.
  Keep per-click conformal for replayed traces that have a per-click calibration
  set.
- **This closes the loop with section 4.** The dominant measured failure is
  grounding and control, not knowledge. An uncertainty model of only what the bot
  *knows* misses most of the failure mass. The uncertainty must be about
  **acquisition**, the three-way resolver verdict of section 1.
