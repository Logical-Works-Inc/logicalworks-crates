# Frontier definition: Benchmarks

Part of [Frontier definition](frontier.md).

## 2. Benchmarks — the boards do not measure the thing the bot is for

**Frontier claim.** Short-horizon web benchmarks are saturated or statistically
unordered; long-horizon is genuinely low; and **no benchmark in the field isolates
UI drift on a stable task**.

**Anchors.**

- WebVoyager: 59.1% → 98.5%, scored by GPT-4V judgment on live sites, so it no
  longer orders the top tier. GAIA: 92.36% for a six-model ensemble, i.e. an
  orchestration artifact, not a model result.
- SWE-bench Verified convergence audit (ADMA 2026, 254 submissions) — the top ten
  share 285 successes. Scaffold choice swings a model by up to **29.8 pp** against
  an 8.8-pp spread across the top thirty, and McNemar separates **none** of the 29
  adjacent top-thirty pairs at α=0.05.
- OSWorld 2.0 (2026) — 108 tasks, ~318 tool calls each. Best binary completion
  **20.6%** against 54.8% partial. The gap between partial and binary *is* the
  long-horizon failure rate. Agent S: **+9.37% absolute** over the OSWorld
  baseline from dual-input observation.
- HORIZON (2026, 3,100+ trajectories, human–judge κ=0.84) — long-horizon failure
  is a **compositional shift**, not a rate drop: planning and memory failures
  displace execution failures as horizon grows.
- Online-Mind2Web — most commercial agents *underperform* the academic SeeAct
  baseline from early 2024 on live sites.
- DynamicGUIBench (ACM MM 2026) — reframes the problem as a POMDP where the
  critical state occurs *between* observations and is unrecoverable from any
  single screenshot.

**Solved.** Nothing that matters here. The boards are instruments for the wrong
quantity.

**Open.** Robustness under UI drift has **no credible dedicated benchmark**. The
closest instruments inject *observable* anomalies, or measure hidden interstitial
dynamics. Nobody measures "the page changed underneath the automation, did the
agent notice and recover," with a success-rate delta attributable to the mutation.

**What it changes.**

- **Build the drift harness as the primary regression gate**, not as a nice-to-have.
  Mutation injection — element moved, renamed, restructured, removed — under a
  fixed task, reporting a success-rate delta attributable to each mutation class.
  This is a gap the whole field has, so owning it pays.
- **Report paired per-task outcomes and comparison-set resolution, never an
  aggregate alone**, per the SWE-bench audit protocol.
- **`Observe` binds to a temporal window with a monotonic sequence number, not a
  screenshot.** A single post-action frame is a POMDP observation, and the
  dominant long-horizon failures are downstream of that.

## 3. Agent architecture — the loop converged; the differentiation moved out of it

**Frontier claim.** The frontier converged on one loop shape — plain iterative
action-observation — with plan-execute surviving as a *mode grafted onto* it and
reflection demoted to a stop condition. The remaining differentiation is entirely
in the surrounding harness: context lifecycle, verification, permissioning,
handoff.

**Anchors.**

- Harness Engineering (Sept 2026) — source audit of 11 production harnesses,
  ~4M LOC. All eleven implement action-observation variants; nine are plain
  iterative; coordinator-worker is only ever an overlay. **No runtime imports
  LangChain, LangGraph or AutoGen, and none retrieves code with embeddings.**
- Sample More, Reflect Less (Aug 2026) — 36 paired token-matched comparisons with
  Holm correction: **no method reliably beats repeated sampling at equal token
  cost**, and all 18 self-inspection comparisons were negative.
- Beyond Compaction (2026) — typed, dependency-linked episode graph with a
  deterministic, LLM-free eviction policy; one session over 89 sequential tasks
  and 80M tokens with no measurable degradation.
- HAS-Bench (2026) — humans and agents as first-class graph participants with
  roles, permissions and **action authority**.

**Solved.** Loop topology. Observation space (structured interface for identity,
screenshot for change, element-index actions). Framework-versus-model — settled by
absence at scale. Self-reflection as a general accuracy lever — settled negative
when self-administered at equal tokens.

**Open.** Long-horizon hidden state (losing constraints across ~318-step horizons)
is the headline failure, not GUI control. **Escalation calibration** — "guess
rather than ask the user" is named by benchmark authors as a top failure mode, and
no production harness ships a policy for *when* to escalate. Interruption and
resumption have no cross-system semantics, and no harness treats **authority** as
the thing being suspended.

**What it changes.**

- **Do not add a `Plan` verb.** Plan-execute is a mode over an action-observation
  base. A plan belongs in `Query` as a *candidate record* under the existing
  `(Auth, input)` seam, with `GrantSet::issue` remaining the only mint.
- **Do not build self-critique into `Evaluate`.** The measured verdict on
  self-inspection at equal tokens is negative-to-neutral. Critique belongs in an
  outer verify-on-stop guard whose verifier is mechanical or tool-grounded —
  which is exactly `forge-md-runtime`'s advisory-versus-promoted split. That makes
  the reflection question an *evidence* question rather than a taste question.
- **Context is a dependency-linked typed episode projection, not a transcript**,
  with deterministic eviction rather than summarization-compaction.
- **The frontier's top open problem is the one `GrantSet` already models.**
  "Guess rather than ask" is an authority-escalation question. A `Query` that
  *requests a grant* makes asking a first-class machine-checkable outcome, and
  human takeover becomes a grant lifecycle event rather than the dialogue state
  machine this architecture deliberately does not have.

The ECS change-detecting schedule is a **stricter factoring** than any of the
eleven audited production harnesses: it separates observation from evaluation
deterministically rather than by prompt.
