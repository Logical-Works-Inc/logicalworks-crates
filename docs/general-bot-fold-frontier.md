# Folding the recorder: the sequence, steps 6 to 14

Pages of this guide: [scope and finding](general-bot-fold.md) · [what is new](general-bot-fold-new.md) · [recorder, settlement, language](general-bot-fold-language.md) · [semantic tier and mapping](general-bot-fold-semantic.md) · [reaching a page](general-bot-fold-reach.md) · [evasion and injection](general-bot-fold-evasion.md) · [sequence 1 to 5](general-bot-fold-sequence.md) · [sequence 6 to 14](general-bot-fold-frontier.md)

6. **The politeness frontier.** ✅ Landed in this change: `frontier.rs` with
   `ConstraintKey`, `Resolved`, `RulesState`, the three-armed `Admission`, a
   validated `PolitenessPolicy`, and the `Frontier` that admits against every
   constraint a request shares. Implementing it revised three of this page's own
   commitments, and each revision is a finding, not a preference:

   - **The verdict is three-armed, not four, and §5.3's three are right.** The
     first draft gave each cause its own arm. That is wrong, and the cause list
     shows why once written out. Upstream failure mechanics are unbounded: a DNS
     `SERVFAIL`, a TLS timeout and a redirect loop on `/robots.txt` all mean *we
     could not look*, and none changes what the scheduler does. The epistemics
     belong in `RulesState` and `Resolved`, which are explicit state machines, and
     the verdict reports only the operational result. The property that makes it
     safe, and the bug it avoids: **nothing provisional may be spelled `Reject`**.
     A terminal drop and a re-queue are different actions. A consumer that must
     inspect a payload to learn which it holds has no verdict.
   - **RFC 9309 supplies a third arrival of the invariant, and RFC 8020 a
     fourth.** *No rules were retrieved* has two opposite answers, decided by why.
     §2.3.1.3's 4xx means unavailable, and "the crawler MAY access any
     resources". §2.3.1.4's 5xx means unreachable, and the crawler "MUST assume
     complete disallow", only *until a fresh, valid file is obtained*. That is why
     `Unreachable` carries an instant and expires. Resolution splits the same way:
     `NXDOMAIN` is authoritative and rejects (RFC 8020), while `SERVFAIL` and
     timeouts are not and defer.
   - **Politeness is keyed on infrastructure, not hostnames, and the delay was the
     wrong control variable.** A per-host limit is blind to what a host is. Two
     hundred Cloudflare-fronted domains are two hundred "independent" polite
     queues, and one reverse proxy sees two hundred requests a second from one
     egress: a Layer 7 flood with a polite-looking config. So a request is
     admitted only when every constraint it shares admits. The controlled
     quantity is an **integer concurrency window**, not a delay derived from
     latency. Scrapy's `AutoThrottle` makes `target_concurrency` a divisor, not a
     bound. It holds instead of backing off on a non-200, never reads
     `Retry-After`, and reads latency as load, so a fast CDN edge cache makes it
     *speed up* under origin strain.

   Not here yet, and named: the *drivers* that fetch rules and resolve names.
   This module models both states and records the observations, but nothing
   performs them. Also §8.1's durable `Pending`/`Done` frontier, whose two-phase
   mark matters once a run can be killed mid-flight.
7. **Declared identity**, wired as a flow property and recorded per run (§5.2).
8. **The `bot.evade` capability**, with the seeded, recorded draw and the grant
   log (§5.4, §5.5). It comes after 6 and 7 on purpose, so the cheap remedies are
   in place before the expensive one is reachable.
9. **The settlement policy.** `Time<Virtual>`-driven, declared per step, with
   non-settlement as a typed terminal outcome (§3.4).
10. **The recorder and its visible-delta compiler** (§3.3), on the ECS substrate.
11. **The agent domain**, closed-enum actions and `FlowBounds` budget (§5.7).
12. **The MCP adapter**, over the four verbs. The protocol edge, `rmcp`, is already
    admitted through `lgwks_deps` (ADR 0015).
13. **Semantic-tier calibration.** `SemanticPolicy::DEFAULT` is `{ threshold:
    0.72, margin: 0.05 }`: declared, not fitted (§3.6). They are constructor
    arguments because the right values depend on the model. Nothing yet *finds*
    them or *measures* whether they are right. Semantic Router, the closest
    production implementation, ships `fit(X, y)`, `evaluate(X, y)` and
    `get_thresholds()` for this. Its own published result is the useful part: a
    five-route healthcare split scores **54.2%** validation accuracy *after*
    fitting. So embedding-similarity routing can be far worse than a threshold's
    confidence suggests. Until this exists, "the semantic tier helps" is an
    assertion with no measurement, and step 3's constants are unexamined.
14. **Precomputed embeddings at the resolver boundary.** `SemanticResolver` embeds
    the utterance and every candidate option on each `resolve`. A flow with *n*
    options costs *n + 1* provider calls per turn, every turn. The same
    implementation's `BaseRouter.__call__` takes an optional `vector` that skips
    encoding and leaves the lifetime decision to the caller. That shape fits here.
    It needs no cache, which the estate forbids without a bound and an eviction
    policy, and it keeps one implementation of the decision, not a second entry
    point to the same work.

One note against a future simplification. Semantic Router's `BaseRouter`
*raises* when its index is not built: the same *could not look* situation that
`Resolution::Degraded` carries as a verdict. Both express it; only one forces the
caller to handle it. `Degraded` looks like a value that could fold into `Absent`,
and it cannot: that is the collapse step 5 exists to prevent.

## 7. What this page does not decide

- Whether the durable queue is a storefront feature or stays out of the
  workspace. That needs the dependency doctrine's ruling, not this page's.
- The scheduler decision in `docs/bot-on-ecs.md` §9. It needed project-owner
  sign-off and no longer does: taken on 2026-09-21 as a first-party `lgwks_std`
  module, with the finding it was taken against recorded in §9.
- Anything about the described platform's licence. Nothing was copied.
