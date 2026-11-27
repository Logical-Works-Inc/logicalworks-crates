# ADR 0088: every CI fetch has a deadline

Status: **accepted**, 2026-11-26.

Stakeholders: the Director; every pull request that waits on the 300 s run wall.
Concerns: a stalled download holds a job past the wall while every test is green.
Requirements: INV-CI-FETCH-1.
Viewpoint: the hosted CI view of `.github/workflows` and `.github/actions`.
Elements and relations: rule 12 of `check-gate-parity` (`check_gate_parity::bounded_fetch`) reads every workflow and
action; the `gate-parity` lane runs it.

## Context

On run 45910843702 (PR #1162) one job's `curl` of the kache release waited 120 s, then GitHub answered 504. The job
took 284 s, and the run walled at 323 s against its 300 s limit. Every test passed. The other jobs fetched the same
asset in under a second. The fetch had `--retry 5` but no `--max-time`, so each try could wait without limit.

## Decision

1. Every `curl` has a `--max-time`. The kache asset gets 30 s a try, with `--retry-all-errors`, which retries a timeout.
2. Rule 12 refuses a `curl` with no `--max-time` or `-m`. It joins backslash continuations, ignores comments, and judges
   each `curl` of a line alone, so one bound never covers the next. It reads every file under `.github/workflows` and
   every `.github/actions/*/action.yml`, so a new workflow is held the day it lands.

## Alternatives rejected

- **A job `timeout-minutes`.** It ends the job at the wall instead of letting the retry run, so the run still fails.
- **A rerun of the run.** Reruns without a cause are banned, and the stall comes back.

## Consequences

- A healthy fetch is unchanged. A stalled one costs at most 30 s before its next try.
- A shell string with ` #` in it hides the rest of its line from the rule.

## Correspondence

Relates to ADR 0086 (kache) and ADR 0041 (the 300 s wall).

## Verification

`sim_bounded_fetch_refuses_exactly_the_unbounded_curls` sweeps 256 seeded commands against a model.
`sim_bounded_fetch_judges_each_curl_alone_and_names_its_line` holds the reader rules.
`sim_bounded_fetch_holds_for_every_committed_workflow_and_action` holds the committed tree.
