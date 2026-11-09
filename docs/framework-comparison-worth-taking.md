# Framework comparison: What is worth taking

Part of [Framework comparison](framework-comparison.md).

## What is worth taking

Ranked by expected value to this project.

**1. Declared observation scope, borrowed from Discord's `intents`.** The bot has
`GrantSet` bounding what an *action* may do, and nothing bounding what it may
*observe*. A declared, enumerated observation intent the runtime enforces — where
capturing an undeclared surface is an error rather than a silent success — closes
the read side of the same gap. This is the single most valuable idea in the
survey and it has no equivalent in the estate today.

**2. Tiered escalation with a measured firing rate.** Already adopted for the
resolver; `spider-rs` confirms it generalises, and the same shape should apply to
re-observation and to journal compaction.

**3. Bounds as first-class, not as configuration.** Capacity on every channel,
deduplication by key, bounded retries, and a run that stops on its own. The
frameworks that scaled all did this; the platforms that did not are the slow
ones.

**4. Model output into a validated type, never free text.** Enforced twice
independently — Crawlee's validated model, `forge-md-runtime`'s advisory
candidate.

**5. One schema, generated surfaces, a cordoned runtime.** Stagehand's shape, and
already this workspace's shape.

## The gap none of them close

Every framework surveyed bounds **how much** work is done and none bounds **what
an action may do**. Discord's `intents` scopes what a bot *receives*; nothing in
the field scopes what it *sends*, carries proof that a specific invocation was
authorized, or records a decision with provenance a third party can verify.

That is not an accident of this sample. The commercial platforms cannot
differentiate on authority because their value proposition is unconstrained
automation, and the open frameworks have not needed it because their users are
the operators.

So the honest conclusion is: **the thesis is correct.** This is one idea,
differentiated on observation surface and decision source, packaged and sold on
hosting and bypass infrastructure. The two axes that would carry real
differentiation — authority bounded at the invocation, and decision provenance
that survives third-party verification — are the two nobody occupies.

Which is a better position than it sounds. It means the bot does not need to
invent a new category to be different. It needs to be the first implementation of
the *same* idea that bounds itself and can prove what it did.
