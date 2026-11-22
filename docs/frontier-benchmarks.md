# Frontier definition: Benchmarks

Part of [Frontier definition](frontier.md).

## 2. Benchmarks: the boards do not measure the thing the bot is for

**Frontier claim.** Short-horizon web benchmarks are saturated or statistically
unordered. Long-horizon scores are low. **No benchmark in the field isolates UI
drift on a stable task.**

**Anchors.**

- WebVoyager: 59.1% → 98.5%, scored by GPT-4V judgment on live sites, so it no
  longer orders the top tier. GAIA: 92.36% for a six-model ensemble, i.e. an
  orchestration artifact, not a model result.
- SWE-bench Verified convergence audit (ADMA 2026, 254 submissions): the top ten
  share 285 successes. Scaffold choice swings a model by up to **29.8 pp** against
  an 8.8-pp spread across the top thirty, and McNemar separates **none** of the 29
  adjacent top-thirty pairs at α=0.05.
- OSWorld 2.0 (2026): 108 tasks, ~318 tool calls each. Best binary completion
  **20.6%** against 54.8% partial. The gap between partial and binary *is* the
  long-horizon failure rate. Agent S: **+9.37% absolute** over the OSWorld
  baseline from dual-input observation.
- HORIZON (2026, 3,100+ trajectories, human–judge κ=0.84): long-horizon failure
  is a **compositional shift**, not a rate drop. Planning and memory failures
  displace execution failures as the horizon grows.
- Online-Mind2Web: most commercial agents *underperform* the academic SeeAct
  baseline from early 2024 on live sites.
- DynamicGUIBench (ACM MM 2026): a POMDP where the critical state occurs
  *between* observations and no single screenshot recovers it.

**Solved.** Nothing that matters here. The boards measure the wrong quantity.

**Open.** Robustness under UI drift has **no credible dedicated benchmark**. The
closest instruments inject *observable* anomalies, or measure hidden interstitial
dynamics. Nobody measures "the page changed under the automation: did the agent
notice and recover?", with a success-rate delta tied to the mutation.

**What it changes.**

- **The drift harness is the primary regression gate.** Inject mutations (element
  moved, renamed, restructured, removed) under a fixed task, and report the
  success-rate delta of each mutation class. The whole field lacks this.
- **Report paired per-task outcomes and comparison-set resolution, never an
  aggregate alone**, per the SWE-bench audit protocol.
- **`Observe` binds to a temporal window with a monotonic sequence number, not a
  screenshot.** A single post-action frame is a POMDP observation, and the
  dominant long-horizon failures are downstream of that.

## 3. Agent architecture: the loop converged, and the difference moved out of it

**Frontier claim.** The field converged on one loop: plain iterative
action-observation. Plan-execute survives as a *mode grafted onto* it, and
reflection is demoted to a stop condition. What still differs is the harness
around the loop: context lifecycle, verification, permissions, handoff.

**Anchors.**

- Harness Engineering (Sept 2026): a source audit of 11 production harnesses,
  ~4M LOC. All eleven implement action-observation variants, nine are plain
  iterative, and coordinator-worker is only ever an overlay. **No runtime imports
  LangChain, LangGraph or AutoGen, and none retrieves code with embeddings.**
- Sample More, Reflect Less (Aug 2026): 36 paired token-matched comparisons with
  Holm correction. **No method reliably beats repeated sampling at equal token
  cost**, and all 18 self-inspection comparisons were negative.
- Beyond Compaction (2026): a typed, dependency-linked episode graph with a
  deterministic, LLM-free eviction policy. One session ran 89 sequential tasks
  and 80M tokens with no measurable degradation.
- HAS-Bench (2026): humans and agents as first-class graph participants with
  roles, permissions and **action authority**.

**Solved.** Loop topology. Observation space (structured interface for identity,
screenshot for change, element-index actions). Framework against model: settled
by absence at scale. Self-reflection as a general accuracy lever: settled
negative when self-administered at equal tokens.

**Open.** Long-horizon hidden state (losing constraints across ~318-step
horizons) is the headline failure, not GUI control. **Escalation calibration**:
benchmark authors name "guess rather than ask the user" as a top failure mode,
and no production harness ships a policy for *when* to escalate. Interruption and
resumption have no cross-system semantics, and no harness treats **authority** as
the thing being suspended.

**What it changes.**

- **Do not add a `Plan` verb.** Plan-execute is a mode over an action-observation
  base. A plan belongs in `Query` as a *candidate record* under the existing
  `(Auth, input)` seam, and `GrantSet::issue` stays the only mint.
- **Do not build self-critique into `Evaluate`.** Self-inspection at equal tokens
  measures negative to neutral. Critique belongs in an outer verify-on-stop guard
  with a mechanical or tool-grounded verifier: `forge-md-runtime`'s advisory
  against promoted split. Reflection becomes an *evidence* question, not a taste
  question.
- **Context is a dependency-linked typed episode projection, not a transcript**,
  with deterministic eviction, not summarization-compaction.
- **The field's top open problem is the one `GrantSet` already models.** "Guess
  rather than ask" is an authority-escalation question. A `Query` that *requests
  a grant* makes asking a first-class, machine-checked outcome. Human takeover
  becomes a grant lifecycle event, not a dialogue state machine, which this
  architecture does not have.

The ECS change-detecting schedule is a **stricter factoring** than any of the
eleven audited harnesses: it separates observation from evaluation
deterministically, not by prompt.
