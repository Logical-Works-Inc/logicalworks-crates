# Folding the recorder: evasion, injection and model output

Pages of this guide: [scope and finding](general-bot-fold.md) · [what is new](general-bot-fold-new.md) · [recorder, settlement, language](general-bot-fold-language.md) · [semantic tier and mapping](general-bot-fold-semantic.md) · [reaching a page](general-bot-fold-reach.md) · [evasion and injection](general-bot-fold-evasion.md) · [sequence 1 to 5](general-bot-fold-sequence.md) · [sequence 6 to 14](general-bot-fold-frontier.md)

## 5.4 Per-execution randomization: the objection, and the answer

`puppeteer-extra-plugin-stealth`, `--disable-blink-features=AutomationControlled`,
per-request user-agent and hardware randomization.

The objection is determinism, and it is narrower than the first version of this
page made it sound. A *declared* user agent is deterministic: §5.2 settles that.
The problem is a fingerprint **re-randomized per execution**. It is a
non-deterministic input to a run that must reproduce. `docs/bot-on-ecs.md` §5
puts determinism on this workspace, and a run whose fingerprint varies cannot be
replayed or diffed against its predecessor.

That is fixable, and the fix is the estate's habit: **seed it from the run,
record the seed, replay the same draw.** A seeded draw reproduces like a
constant, and keeps what a constant loses: two flows do not present an identical
client.

It also settles a contradiction the first version missed. `ElementFacts`
fingerprints an element. A bot that randomizes what the page observes about *it*,
while scoring what it observes about the page, measures two different worlds. A
recorded seed makes both sides readable.

## 5.5 Evasion: a granted, recorded capability

Where a polite, declared, session-holding crawler is still refused, evasion is
the remaining remedy. It is **built**, under three conditions that keep it
compatible with the security case:

1. **It is a capability, not a default.** `bot.evade` is granted through
   `GrantSet` like every other side effect. A bot not given it is refused at
   build time, and the grant is re-proved on every call.
2. **It is recorded in the run.** A grant *is* a declaration: the run states that
   it evaded, so an auditor reads it from the same document that lists every
   other authority the run held. That keeps `docs/security-posture.md` true. The
   claim was never "the bot cannot evade"; it was "the run is declarable", and a
   recorded grant keeps exactly that.
3. **It is never ambient and never silent.** A run that used it says so. A run
   that did not, cannot have.

The residual risk belongs in the record. A reviewer will ask about the evasion
capability first, and the answer is the grant log, not an argument. Refused
outright: evasion that is unconditional, unrecorded, and indistinguishable in the
artefact from an ordinary run. A launch flag gives you exactly that.

## 5.6 Code injection, and the locus distinction that makes it coherent

`page.exposeFunction`, `addInitScript` and in-page analyzer injection are
**permitted**, for the reason the security case already draws. The allow-list in
`docs/security-posture.md` constrains what runs in **the consumer's browser**.
The automation browser is a different locus, owned by the estate, and injecting
a recorder into it is the normal mechanism.

The rule that survives the fold: the two loci are never conflated. An
`ExecutionLocus` is fixed when a session opens. A session that drives the
estate's automation browser may not be the session that observes a consumer's
page. Injection into the second is what the whole design exists to avoid. So the
fold adds capability, not ambiguity, while the locus stays explicit.

## 5.7 Free-text model output controlling the browser

The described platform gets most of this right. It forces a strict JSON grammar
and enumerates the actions (`click`, `type`, `press`, `select`, `scroll`, `wait`,
`done`). The fold keeps that and closes the last gap: the JSON is parsed into a
**closed Rust enum**, not a `String` action name. An unrecognized action is a
parse error, not a no-op. A model that emits `"submit"` fails loudly, and never
silently does nothing.

The step budget is explicit too. `FlowBounds` already carries one, and the
described platform's "max 20 steps" is the same parameter. What is added:
exhausting it is a typed terminal outcome, not a silent stop.
