# Requirements F. Evidence

Part of [the product requirements](../../REQUIREMENTS.md), which state how a requirement changes.

## F. Evidence

### R13. Every claim carries its axis, its gate and its evidence class.

> A readiness or performance claim MUST name the axis it addresses, the gate it
> is measured against, and whether its evidence is measured, proved, argued or
> unmeasured. Losses MUST be published at the same volume as wins. Exclusions
> MUST be named with their direction of bias.

**Invariants.** No number without a method. No relative claim without an
absolute. No exclusion without a stated bias. A comparator is the null
hypothesis (a hand-rolled loop doing identical work) or a named rival with a
stated reason it should win.

**Callers.** Every release claim. Every readiness document.

**Falsifier.** A readiness score with no evidence class, or a performance claim
with no absolute number and no named exclusions. The benchmark fairness gate in
`bench/README.md` is the model: it counts effects *and* condition evaluations,
and a mismatch aborts the run rather than printing a ratio.

**Status.** `met` as a rule from now on, measured by
`docs/production-readiness.md` and `bench/README.md`. It is a rule this
repository has already broken and corrected.
