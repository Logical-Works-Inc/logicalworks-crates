# Guidance runner specification: Element resolution

Part of [Guidance runner specification](guidance-runner-spec.md).

## Design constraints on element resolution

These bind all future element-resolution work, including the deferred interface
model. They are not preferences. Each follows from a measured result recorded in
[`frontier.md`](frontier.md), and each must be enforced **structurally** — by the
types and the seams — rather than by convention or reviewer discipline.

**1. The resolver never routes an uncertain resolution to a human for
element-level confirmation.** `Ambiguous` is a terminal observation state. The bot
re-asks about the *task*, or refuses, or hands off with the partial evidence. It
never presents a candidate element for a person to accept or reject, and it never
displays two versions of an interface side by side for comparison.

**2. The recognition representation is never mutated from user input.** There is
no "this selector stopped matching, drop it" learning loop, and no authorised path
by which a human judgement edits what the resolver considers.

**3. No fleet-side corpus.** No repository collects observed interface structure
from multiple client devices, and no recognition model is trained or retrained
from one.

**4. Structural resolution only; a visual path requires Set-of-Mark.** If a
visual component is ever added, the model selects an identifier the resolver
minted and never emits or infers a coordinate. Anchor-by-proximity and geometric
relationship inference — distances, angles, line segments, whitespace heuristics —
are excluded outright.

**5. Failure is refusal, not repair.** A resolution that cannot be verified
produces `Ambiguous` or `Absent`, and the run terminates, re-asks, or escalates
with its evidence. The repair path *is* the refusal path.

### Why these are structural, not stylistic

The measured grounding result is that "the element moved" and "the element is
gone" have *overlapping* score distributions: deleted-element decoys score
`[0.665, 0.955]` against true drift at `[0.749, 0.874]`. No threshold, margin
or filter separates them, and the strongest available semantic reranker was
*unanimously wrong* on absent elements while being right on present ones.

Constraint 1 follows directly: if no score can distinguish the two cases, then a
human presented with a candidate cannot be given the information needed to
distinguish them either, and asking them to adjudicate converts a bounded failure
into an unbounded one. Constraints 2 and 3 follow from the same overlap — a
learning loop trained on that signal is being trained on noise. Constraint 4
follows from the measurement that structural resolution outperforms visual
resolution on exactly this problem. Constraint 5 is the consequence of all four:
since guessing cannot be made reliable, the only honest output is a refusal that
carries its own evidence.

Together these make `Ambiguous` and `Absent` **first-class outcomes with no
downstream repair path**, which is what gives `ElementRef` its guarantee — it is
minted only from `Resolved`, so nothing guessed can reach `Execute`.

## Evidence required before merge

For each workstream: the test command and its result, the clippy result, the fmt
result, and the exact list of files changed. No claim without the command that
produced it.
