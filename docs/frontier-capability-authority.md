# Frontier definition: Capability authority

Part of [Frontier definition](frontier.md).

## 6. Capability authority — the bet is supported, and it is one argument check short

**Frontier claim.** Capability-scoped authority for agent tool calls is a real
research field, and the frontier has moved past "hold a capability" to
*invocation-scoped, argument-bound, attested* authority. The decisive negative
result: every major agent framework ships capability gating by default and **none
ships a fail-closed per-call authorization decision**.

**Anchors.**

- ScopeGate (arXiv 2606.28679, Jun 2026) — audit of LangChain/LangGraph,
  LlamaIndex and Stripe Agent Toolkit: all three provide capability gating by
  default; **none provides a deterministic fail-closed per-call value
  authorization gate**.
- ChainCaps (ICML 2026 workshop) — budgets propagate by **intersection**; names
  "permission laundering" — an agent can pass every per-tool check and still
  produce an unsafe end-to-end effect. Cuts attack success from 25–68% to 0–4.8%.
- MiniScope (Dec 2025) — least-privilege enforcement at 1–6% latency overhead.
- Proof-Carrying Agent Actions (Jun 2026) — an action certificate with five
  checkpoints and explicit approval enforceability classes.
- ACLE-MCP (arXiv 2609.02690) — short-lived sender-constrained **leases** binding
  expected workload, freshness, operation, object and parameter bounds, consumed
  at a provider-side execution gate.
- **Ghost Field Exfiltration (SBSeg 2026)** — no prompt injection required: a page
  renders a small visible form and hides extra inputs in the DOM, and Claude for
  Chrome fills them with user data at **19% (Opus 4.6) to 64% (Haiku 4.5)** full
  exfiltration.
- Classical spine: Proof-Carrying Authorization (Bauer/Appel), Macaroons (NDSS
  2014) for attenuation by contextual caveat, Redell & Fabry (TSE 1979) for
  revocation.

**Solved.** Per-tool permission checks are insufficient — permission laundering is
now a named, measured failure mode. Attenuation must be monotone: authority may
lose but never gain through composition. "The framework exposes a tool" is not
authorization, with an audit proving it across the three dominant frameworks.

**Open.** Argument-bound, fail-closed authorization at dispatch — nobody has
shipped a general per-call value-level policy decision point. Revocation: the
agent-era answer is to *avoid granting* rather than to revoke; no system has a
"revoke now, the in-flight action stops" primitive. And **bounding a UI element
rather than an API operation** — a DOM field is not an addressable capability in
any published system, and model-layer defenses fail because they key on
sensitive-data category recognition rather than structural detection of abuse.

**What it changes.**

- **`Auth` proves a grant exists, not that this call's arguments are authorized.**
  `execute_action` needs a ScopeGate-shaped, argument-bound, **default-deny**
  decision at dispatch, in addition to the capability. This is the decisive
  near-term gap.
- **`GrantSet` combination must be intersection-only at the type level.** If two
  weaker `Auth`s can ever union into a stronger one, permission laundering is
  reintroduced above the type system.
- **Answer "granted yesterday, action today" with leases, not revocation.** Give
  `Auth` an expiry and an invocation-scoped binding to operation, object and
  parameter bounds, and consume it linearly rather than holding it.
- **Treat the DOM as a sink.** If `poll`/`query` can read a page into context and
  `execute_action` can write a value into a field, **the field's identity belongs
  in the authorization object**. Ghost Field Exfiltration proves the
  model will not catch a hidden input, and the substrate is the only layer that
  can. This is the clearest genuinely-unmapped opening in the whole sweep, and it
  is exactly the "point the bot at four or five things and declare invariants"
  idea, made enforceable.
