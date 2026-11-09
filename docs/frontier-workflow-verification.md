# Frontier definition: Workflow verification

Part of [Frontier definition](frontier.md).

## 5. Workflow verification — the validator I specified implements one of three known properties

**Frontier claim.** Control-flow static validation is a closed, citable discipline:
the workflow-net soundness triple is settled, decidable, and checkable on
realistic models in under 500 ms with automatic repair. The live frontier moved to
SMT/SAT verification of *data-aware* constraints and to static verification of
*agent* workflow graphs — which is what this design is.

**Anchors.**

- van der Aalst et al., *Soundness of workflow nets* (Formal Aspects of Computing
  2011) — the canonical taxonomy: **option to complete**, **proper completion**,
  **no dead transitions**, with a decidability map for every relaxation. Soundness
  for nets *with reset arcs* is proved **undecidable** — the standard example of
  where static checking stops being possible at all.
- Pesic & van der Aalst, *DECLARE* (EDOC 2007) — the canonical declarative
  constraint language: a fixed LTL template set (existence, absence, response,
  precedence, chain-response) that *is* the established way to state cross-step
  constraints.
- De Giacomo et al. (TOSEM 2022) — finite-trace semantics (LTLf/LDLf), the correct
  semantics for a conversation that terminates rather than an infinite trace.
- Maggi et al., *Runtime Verification of LTL-Based Declarative Process Models*
  (RV 2012) — compile each template to an automaton and monitor the live event
  stream instead of exploring a state space.
- Burattin et al. — MP-Declare: activation and target conditions bound to data
  variables, so a rule spans an arbitrary pair of steps, enforced over a joined
  execution model rather than per-screen.
- AgentProof (arXiv 2603.20356, 2026) — the agent-workflow analog, and the closest
  published system to the flow document: structural checks (reachability, dead-end
  detection, tool-declaration coverage, router-edge validation, entry/exit
  structure) plus an LTL DSL compiled to DFA monitors.

**Solved.** The soundness set is settled and decidable; the constraint language is
not in dispute; the semantics for a terminating conversation is settled.

**Open.** No published **handoff criterion** for when static verification should
yield to runtime monitoring — the stated reason is a *semantics gap* (the model's
language is not the implementation's language), which is qualitative, not a
threshold. Data-aware verification does not scale generally: multi-perspective
conformance over unbounded data is undecidable, and every practical system quietly
bounds the data domain. Agent-workflow verification has **no soundness theorem
yet** — pre-consensus, Python-only, validated by example.

**What it changes.**

- **Name the soundness triple in the flow validator.** "Refused on unreachable
  nodes" *is* no-dead-transitions. The two the spec does not yet name are the ones
  that catch a malformed flow. They are **option-to-complete** (every reachable state can
  still reach an end) and **proper-completion** (the end state is the *only*
  reachable terminal state). A flow able to reach `End` while another branch is
  live violates it. Add both as named checks the invariant register can cite.
- **A closed predicate language over scalars cannot express the register's modal
  cases.** "Must never happen" and "must eventually happen" are temporal, not
  node-local. They belong in the Declare template set with finite-trace semantics,
  compiled one-DFA-per-invariant and run as a **monitor** alongside the verbs.
- **The register accepts exactly two enforcement kinds** — `static-check` and
  `monitor` — and refuses any invariant whose enforcement is review or intent.
  It stays decidable precisely as long as the predicate language stays closed over
  scalars with a **bounded domain**; the moment an invariant quantifies over
  unbounded runtime values, no gate can refuse it and the register degrades to
  documentation.
- **Do not model-check the runtime.** The semantics gap is the argument for the
  monitor seam, not a defect to close.
