# Folding the recorder: recorder, settlement and language

Pages of this guide: [scope and finding](general-bot-fold.md) · [what is new](general-bot-fold-new.md) · [recorder, settlement, language](general-bot-fold-language.md) · [semantic tier and mapping](general-bot-fold-semantic.md) · [reaching a page](general-bot-fold-reach.md) · [evasion and injection](general-bot-fold-evasion.md) · [sequence 1 to 5](general-bot-fold-sequence.md) · [sequence 6 to 14](general-bot-fold-frontier.md)

## 3.3 The recorder, and what a compiler over it may not do

Recording is `Observe` plus an acquisition system on the ECS substrate. The
snapshot the browser already produces is the observation, and `ElementFacts` is
its Rust-side type. No new verb and no new transport.

The described platform's generator is an *optimizing compiler* over the recorded
actions. It merges an action into an existing guard block when the page state
already matches (overshadowing elimination). It injects synchronization barriers
between state-mutating actions. Both are good, and both are silent rewrites: the
executed flow differs from the recorded flow.

Refuse that. `docs/guidance-runner-spec.md` states that the flow document is an
external contract and is additive-only. The security case in
`docs/security-posture.md` rests on the run being *declared*, so an auditor can
read what a bot will do. An optimizer that rewrites the authored document breaks
that, and invisibly: the document a reviewer reads is not the one that ran.

So **the generator optimizes, and records what it did.** Every merge and every
inserted barrier is a declared node or an annotation on the flow, never applied
behind the author's back. The compiled and authored artefacts differ by a
*visible* delta, and that delta is the review surface. A compilation that cannot
be a declared delta does not happen.

## 3.4 The settlement policy, and why it is declared, not heuristic

The described platform picks its wait strategy from the downstream operations. A
DOM-only scrape navigates with `domcontentloaded` to skip subresources. A
screenshot upgrades to `networkidle` and waits for images to settle. To absorb
single-page-application updates, it polls matching element counts until they
hold steady for a window (default 400 ms).

The observation is right and the mechanism is wrong here. A settlement window
read off the wall clock is a non-deterministic input. `docs/bot-on-ecs.md` §5
puts determinism on this workspace, not on the substrate, and supplies the
remedy: `Time<Virtual>` as the clock root, so a test runs ten thousand ticks with
no wall clock.

The failure path is the one that matters. "Poll until stable, else retry" is how
a flaky test is born: the retry hides that the page never settled. This is
correction 4 in `docs/guidance-runner-spec.md` §"Corrections from the brittleness
sweep": *flakiness is a typed outcome with a quarantine, never a silent retry*.
So non-settlement is **a typed terminal outcome**, not a retry and not a timeout
error. The run reports that the page did not reach the declared stable state,
with the observed counts, and the flow's author decides what that means.

The wait strategy is also a **declared property of the step** ("this step needs
layout", "this step needs content"), not a heuristic the engine applies. With a
heuristic, two runs of one flow can take different navigation paths, which the
determinism section forbids.

## 3.5 Language understanding: **landed in this change**

The described platform forces a model's output into a strict JSON grammar. It
says nothing about understanding what a *person* typed, which is the harder half.
A recorded flow asks questions, and the answer arrives as free text with the
spelling, abbreviations and house vocabulary the person brought.

`language.rs` is that layer. It is built like classic autocomplete and
command-and-control recognisers: a **tiered match over a lexicon**, not a model.
Three ordered tiers: `Exact` (normalized equality, or a learned alias),
`Phonetic` (Soundex equality) and `Fuzzy` (a blend of `Jaccard` token overlap and
bounded `EditDistance`). They are ordered, not summed, because an exact match
must short-circuit, and a weighted sum cannot express that.

The resolution reports **which tier won**, and that matters. An `Exact` miss is a
learned alias that points at the wrong option. A `Phonetic` miss is Soundex's
English bias. A `Fuzzy` miss is a threshold. Those are three repairs, and a bare
`usize` names none of them.

### The linguistic-change question, answered in two parts

"Linguistic change" is two unrelated problems, and one mechanism cannot cover
both:

- **Spelling and sound**: typos, transliteration, regional spelling. A property
  of the *letters*, handled statically by the phonetic tier, with no learning.
- **Vocabulary**: slang, house terms, abbreviations. No string distance derives
  `"the usual"` from `"Repeat last order"`. A property of the *group's usage*,
  handled by `LanguageResolver::learn`, which records an alias when a resolution
  is confirmed.

The second is a **readable table, not a fitted model**. Every alias is a row a
reviewer can read, and removing one deletes a row. A learned scorer would adapt
as well and could not be audited, and the security case rests on a declarable
run.

Two limits are stated here, not hidden in the code. The diacritic fold is a
bounded table (Latin-1 Supplement and Latin Extended-A), because this crate may
not take a Unicode dependency. A character outside it passes through and degrades
to the fuzzy tier. Soundex keeps the first letter verbatim, so `Smith`/`Smyth`
collapse but `cat`/`kat` and `Catherine`/`Kathryn` do not. That is the
conservative side: in a resolver, a false positive silently selects the wrong
option, while a false negative only re-asks.

### The resolver seam changed shape

`Resolver::resolve` returned `Option<usize>`, the two-way collapse §3.1 exists to
remove. An answer that fits two options equally was reported as *unrecognized*,
so the session re-asked the full list and the same answer resolved the same way
forever. It now returns `Resolution` (`Resolved` / `Ambiguous` / `Absent`).
`Session::answer` narrows an ambiguous re-ask to the options still in play: a
strictly smaller question, so it terminates.

`KeywordResolver` is deleted, not kept beside the new one. It is a strict subset
(the same token tier and nothing else), so keeping both is two implementations of
one job. That is a breaking public-API change for the next release, which the
standing constraint reserves for a separate step.
