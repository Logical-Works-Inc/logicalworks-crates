# Guidance runner specification: Frontier sweep

Part of [Guidance runner specification](guidance-runner-spec.md).

## Corrections from the frontier sweep

The three workstreams above were specified before the literature sweep in
[`frontier.md`](frontier.md) completed. They remain the right shape, and two of
them are now under-specified. These deltas are recorded rather than silently
folded in, because the workstreams were built against the original text.

**Workstream A is a recall stage, not a complete resolver.** Similo's measured
88.0% on real drifted pages is the deterministic default path; a learned reranker
over the top-N takes failures from 70 to 39. So the module is correctly scoped as
written — but it must not be presented as the answer to element resolution. Its
output feeds a cascade.

**Workstream B's validator implements one of three canonical soundness
properties.** "A node unreachable from `entry`" is *no-dead-transitions*. The two
that catch a genuinely malformed flow, and that the acceptance criteria do not
name, are:

- **option-to-complete** — every reachable state can still reach a terminal
- **proper-completion** — the terminal state is the *only* reachable terminal
  state, so a flow able to reach `End` while another branch is live is refused

**Workstream B's resolver needs a posterior and a margin, not a bare
`Option<usize>`.** The measured, principled rule is three-way: above an accept
threshold, advance; ambiguous, re-ask the same node; below an abstain threshold,
terminate. The re-ask counter is per-node and bounded, and the repair **escalates
in form** — verbatim repeat, then narrowed options or explicit confirmation of the
top hypothesis, then terminal — because recovery rates fall off after about the
second repair.

**Two terminal outcomes need a distinct source.** `Refused` must be reachable from
a scope predicate evaluated *before* resolution, and `HandedOff` must carry the
partial evidence and the named unverified constraint. Handing a task back as
though the bot had never been involved produces a measurably worse human error
profile (Jabbour et al., N=259), so a bare target is not sufficient.

**Workstream C's register needs exactly two enforcement kinds** — `static-check`
and `monitor` — and must refuse any invariant whose enforcement is review or
intent. It must also **reject non-monitorable constraints at load**: a
`G(r → F a)`-shaped property has no good or bad finite prefix and can only ever be
unverified. Decidability holds only while the predicate language stays closed over
scalars with a **bounded domain**; an invariant quantifying over unbounded runtime
values cannot be gated at all, and the register degrades to documentation.

**Deferred, and now the highest-value work in the sequence:** the element model.
It is named out of scope above, and the sweep confirms it is the correct next
step, with one addition. The resolver returns a **typed three-way**
(`Resolved` / `Ambiguous` / `Absent`) and the element reference is minted *only*
from the resolved case, so a healed or guessed element has no type-level path into
execution. The moved-versus-gone distinction provably cannot be a scalar
threshold, so this is not a stylistic choice.
