# ADR 0072: a Landlock allowlist holds 4,096 rules and 1 MiB of path bytes

Status: **accepted**, 2026-11-22.

Stakeholders: the Director; Larry, which confines each session command with a `LandlockRules` allowlist; an operator
whose home folder holds many entries.
Concerns: Larry keeps its state directory unreadable by walking from `/` to it and granting every sibling at each
level. A home folder with more than about 60 entries passed the 64 rules the list held, so a common Linux layout
could not start a session (larry#382).
Requirements: INV-BOT-200.
Viewpoint: one confined spawn on Linux, from the rule list to the `setpriv` argv to the kernel ruleset.
Elements and relations: `rt::process::LandlockRules::{MAX_RULES, validate}`, `rt::process::confine::landlock`,
util-linux `setpriv --landlock-rule`.

## Context

Each rule is one `--landlock-rule path-beneath:<access>:<path>` argument pair to `setpriv`, and one kernel rule. The
kernel puts no small limit on the rules in one ruleset; it takes rules until memory runs out. The real limit is
`execve(2)`: argv and the environment together must fit `ARG_MAX`, a quarter of the stack limit, 2 MiB at the
default 8 MiB stack, or the spawn fails with `E2BIG`. One argument may not pass `MAX_ARG_STRLEN`, 128 KiB.
`Confinement::deny_beneath` does not fit Larry: it hides the whole state directory, but the command must read the
session copy under it.

## Decision

1. **`LandlockRules::MAX_RULES` is 4,096.** A 500-entry home folder walked from `/` needs about 520 rules, so the
   bound holds it with margin.
2. **The paths of one list sum to at most 1 MiB.** With the fixed text of each pair (under 40 bytes, so 160 KiB at
   4,096 rules), the argv stays under 1.2 MiB, inside the 2 MiB `ARG_MAX` with room for the environment.
3. **Both bounds are refused before the fork** as `InvalidInput`, and the message names the bound and the count.

## Alternatives rejected

- **No bound, so the kernel decides.** A list of long paths would then fail at `execve` with `E2BIG`. That limit
  depends on the stack limit of the host, so one list could start on one host and fail on the next. Reverse this
  if Landlock gains a way to pass rules that is not argv.
- **Keep 64 and make Larry grant less.** The walk must grant every sibling to keep the state directory unreadable;
  fewer grants either leak it or refuse the session.

## Consequences

- An allowlist of up to 4,096 short paths starts. A list of long paths holds fewer rules: 256 rules of 4,096 bytes
  fill the byte bound.
- Larry reads its bound from `LandlockRules::MAX_RULES` once this release is on crates.io, and keeps no copy.

## Correspondence

Refines the allowlist bound of #337. Answers #1114 and larry#382.

## Verification

`crates/lgwks-bot/tests/it/sim_landlock.rs`: `every_bound_admits_its_maximum_and_refuses_one_past_it` (4,096 rules
pass, 4,097 are refused by name, 1 MiB of path bytes passes and one byte more is refused),
`a_thousand_rule_allowlist_is_applied_by_the_kernel` (Linux) and the seeded `shape_band_*` sweep against the model.
`crates/lgwks-bot/examples/landlock_cost.rs` prints the spawn time at p50, p95 and p99.
