# Folding the recorder: what is genuinely new

Pages of this guide: [scope and finding](general-bot-fold.md) · [what is new](general-bot-fold-new.md) · [recorder, settlement, language](general-bot-fold-language.md) · [semantic tier and mapping](general-bot-fold-semantic.md) · [reaching a page](general-bot-fold-reach.md) · [evasion and injection](general-bot-fold-evasion.md) · [sequence 1 to 5](general-bot-fold-sequence.md) · [sequence 6 to 14](general-bot-fold-frontier.md)

## 3. What is genuinely new

Four things have no artefact here yet. Everything else in §4 is already present
or is a domain implementation.

### 3.1 The interface model: **landed in this change**

`docs/guidance-runner-spec.md` deferred *"the interface model (element
entities, recognition-vector components, acquisition systems) on the ECS
substrate"*, blocked on Workstream A. Workstream A has landed
(`lgwks_std::similarity`: `EditDistance`, `Jaccard`, `PathSimilarity`,
`Geometry`, `Weighted`), so the first two thirds are implemented:

- `ElementFacts`: element entities. The facts a snapshot reports about one
  element, with identity (`tag`, `identity`, `frames`) apart from mutable content
  (`text`, `bounds`). `docs/bot-on-ecs.md` §4 measured that split as
  load-bearing.
- `IdentityComponent` / `PathComponent` / `TextComponent` / `GeometryComponent`:
  recognition-vector components. Each projects one field of `ElementFacts` onto a
  `lgwks_std::similarity` metric, so unlike metrics compose in one `Weighted`.
- `RecognitionVector::fingerprint()`: the shipped opinion. Weights `1/2`
  identity, `1/4` path, `1/8` text, `1/8` geometry. Powers of two, so they sum to
  exactly `1.0` in binary floating point, not by rounding luck.
- `Recognition`: the verdict.

Two facts are decided before scoring, not weighted: an element in another
document, and an element of an incompatible tag. A fact absent on either side (no
identifying attribute, no text, no structural path) contributes nothing, not the
`1.0` its metric returns for empty against empty. The weights are not
renormalized around the gap, so identity and path together are `3/4`, and both
are required to reach the `3/4` threshold. GitHub issue #39 filed this: two
fields missing on *both* sides scored as agreement. An unrelated element with
another tag then resolved against a target that had nothing to identify it by.

`RecognitionVector::recognize` returns `Resolved` / `Ambiguous` / `Absent`, and
**this is the decision the whole fold rests on.** The described platform resolves
an element to a single best candidate by similarity distance. A best-candidate
resolver cannot report *"two elements matched equally well"*. It returns the
first. The run clicks the wrong control, or clicks nothing and reports a timeout. `Ambiguous` makes that state representable, and the margin requirement
makes it reachable: the best candidate leads the runner-up by a declared amount,
or the caller re-asks and does not commit.

The workspace has now reached this invariant three times:

| Arrival | Statement |
|---|---|
| `docs/bot-on-ecs.md` §8.1 | a single-bit seen-set cannot distinguish "done" from "never started" |
| `BotError` | no variant distinguishes "the effect did not happen" from "it may have happened" |
| this module | no two-way verdict distinguishes "nothing matched" from "several matched" |

One invariant, three arrivals. The first and third landed here. The second landed
in §6 step 5, and its two halves are one law, not two patches.

### 3.2 The locator ladder and selector synthesis

The described platform generates an ordered ladder for each selected element:
stable `id`, then `data-testid`, then ARIA role and accessible name, then visible
text, then a class path. It falls back down it.

This slice is landed (`interface.rs`, step 4 below). A ladder is walked against
candidates, so each candidate offers the anchors the ladder names, and
`ElementFacts` carries them (`with_anchor`, `anchor`). The shape is small:

- `Anchor`: a closed enum (`Id`, `TestId`, `Role`, `Text`, `ClassPath`) with an
  explicit `rank()`. Explicit, not a derived `Ord`: a derived order puts the
  strongest variant first *or* last by declaration order, and a reader should not
  have to resolve that coin-flip.
- `Ladder`: anchors sorted strongest first, deduplicated, constructible only
  non-empty. An empty ladder is an error, as `Weighted` treats an empty component
  set.
- `RecognitionVector::recognize_with_ladder`: walk strongest first, and return the
  *strongest* non-resolved verdict observed. An `Ambiguous` at a strong anchor is
  never silently retried at a weak one: a weak anchor that succeeds where a strong
  one was ambiguous is a likely mis-match, not a recovery.
