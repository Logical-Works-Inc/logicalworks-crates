# Frontier definition: Task mining

Part of [Frontier definition](frontier.md).

## 4. Task mining and record-and-replay, and the finding that decides the evidence model

**Frontier claim.** The field gave up "infer the one true workflow from one
trace". The defensible position: induce a program from a **corpus**, admit each
ordering edge only with an auditable evidence tuple, validate on held-out traces,
and **refuse to emit when intent is under-determined**. Invariants that were
never in the trace are not mined. They are recovered by declaring typed
preconditions and postconditions against the system of record.

**Anchors.**

- POWL, *Partially Ordered Workflow Language* (BPM 2023) and *Inductive Mining with
  Choice Graphs* (BPM 2025): the block-structure limit of Inductive Miner is
  closed. Discovery now emits sound-by-construction partially-ordered models.
- TraceCompiler (arXiv 2608.02680, Aug 2026): an edge is admitted only when the
  consumer argument holds a value **uniquely attributable to an earlier producer**.
  **0.928 precision / 0.943 recall** over 15,775 def-use edges, against **0.711 F1
  for adjacency** and 0.712 for frequency-thresholded directly-follows. Ambiguous
  edges are marked *suspected* and impose no order. It **refuses to compile** when
  an irreversible side effect is under-determined.
- Inducing Task Models from Computer-Use Traces (arXiv 2608.20319): 0.974
  agreement recovering interleaved concurrent tasks, 74.9% of execution steps
  reconstructed, +30.0% held-out task accuracy over the strongest baseline.
- **OpenAdapt** (production OSS, launcher `v1.16.0` / Flow `1.34.0`) ships the
  same thesis. In its own words, a person or a watched agent "authors a
  demonstration" that compiles into a program. A `--break-it` run halts when an
  independent read of the record store disagrees with the screen, even though
  every on-screen check passes (`README.md`). *Unsourced at this version:* the
  quote *"one demonstration is evidence, not a specification"*, and an `induce`
  command that needs at least two recordings. Neither is in the `v1.16.0` README.
  Its CLI compiles one recording (`openadapt flow compile rec --out bundle`), and
  its compiler and runtime now live in `openadapt-flow`. The refusal principle
  survives; the command and the quote stay here only as the historical claim.
- AgentLTL (arXiv 2607.02599): FO-LTL procedural rules over traces give a
  deterministic **judge-free** compliance score, usable as an online
  pre-execution gate. +38 / +17.5 pp on held-out patterns, unseen tool names
  included.
- HybridSimilo (EMSE 2026, arXiv 2505.16424): locates **98.8%** of elements with
  broken locators, on a benchmark 12× Similo's and 23× VON Similo's. Structural
  beats purely visual.
- **A 90-replay fault study through a real persistence boundary**: screen
  verification silently mishandles **5 of 7 transactional fault classes**
  (duplicate submission, optimistic-UI phantom success, partial save, stale
  overwrite, double-click). In each, the recorded pixels match *and the screen
  shows success*.

**Solved.** A sound workflow graph from a log: soundness is now a construction
guarantee, not a post-hoc check. The alpha-algorithm class of problems. Variant
handling by exhaustive enumeration, replaced by mining the constraints all paths
satisfy. Recovering a broken *locator* on a changed page.

**Open.**

- There is **no measured quality metric for recovered invariants on UI traces**.
  Business-process mining has fitness and precision machinery, but nobody has
  published precision and recall for "the invariants a recorded enterprise web
  task must satisfy".
- The induction objective is **unfalsifiable as stated**. Fitness, precision,
  generalization and simplicity cannot all be maximized at once, so every 2026
  system falls back to leave-one-out replay. That is a *consistency* check on the
  same distribution, not a correctness check.
- Essential against incidental is **not recoverable from one trace, by proof**:
  "one frame cannot tell you which text is invariant", because clocks and
  counters look like dates of birth.

**What it changes.** This section should reorder the roadmap.

- **The five-in-seven result ends screen-based verification as the final
  evidence.** Render-drift self-healing cannot touch a duplicate submission or an
  optimistic-UI phantom success, because the screen is *correct* in those cases.
  The `Execute` postcondition is read from the **system of record**, with a
  three-valued CONFIRMED / REFUTED / INDETERMINATE verdict. The UI tells the bot
  what to do; it cannot tell the bot what happened.
- **Induction is a `Query` over an `Observe` corpus, never a compile of one
  recording.** An edge whose producer→consumer attribution has no proof is carried
  as unordered and suspected, never guessed by frequency: 0.711 F1 is no fallback
  for 0.928 precision.
- **Refusal is a first-class `Execute` outcome, and under-determination is the
  common case, not an error path.** An earlier revision credited this to
  OpenAdapt's `induce` exiting nonzero with no bundle. That command is unsourced
  (see above); the behaviour is right regardless.
- **Essential against incidental is a `Query` question, not a mining question.** A
  step is essential if and only if eliding it changes an externally observable
  effect. That is decidable against declared effects, and not from the trace.
- **Coverage, not accuracy, binds.** OpenAdapt's identity gate arms only 4–7 of 12
  clicks in its own live bundles. Disclosed is not closed.
