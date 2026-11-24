# ADR 0085: a sliced shard runs each slice once

Status: **accepted**, 2026-11-24.

Stakeholders: the Director; a contributor whose pull request waits on the Linux shards.
Concerns: a shard cancelled at its cap on a slow runner; a slice that no job runs while every job is green.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the tests it runs.
Elements and relations: the `tests_linux` matrix of `.github/workflows/ci.yml`; rule 9 of `check-gate-parity` in `crates/lgwks-deps/src/bin/check_gate_parity/partitions.rs`.

## Context

The Director, 2026-11-24: "Why the fuck do you have release and CI that has taken so long."
On main, bot-it-g7 takes 65 s of tests. Every other bot-it group takes 20 to 47 s.
On run 45399189913 a slow hosted runner built it in 125 s, then cancelled it at its 5 minute cap at test 795 of 1,722.

nextest's `--partition slice:N/M` runs only the Nth of M parts. A row dropped, a slice named twice or two cuts of one shard leave tests that no job runs, and each job still ends green.

## Decision

1. **bot-it-g7 runs as `slice:1/2` and `slice:2/2`**, as the scale group runs, so each slice holds about 33 s of tests.
2. **Rule 9 holds every sliced shard to its slices.** The rows of one shard, by `archive` and `it_group`, name each of `slice:1/M` to `slice:M/M` once. A slice that is not `N/M` is refused too.

## Alternatives rejected

- **A longer `timeout-minutes`.** Every gate holds under 5 minutes, and a longer cap hides the slow shard.

## Consequences

- One more Linux job a run. It builds the same archive, so it adds a build, not tests.
- A new split names its slices once, or the `gate-parity` lane refuses the workflow.

## Correspondence

Refines the first half of rule 9 (#923), which makes each sliced step pass `--partition`.

## Verification

`every_sliced_archive_runs_each_slice_once` drives the shipped `check-gate-parity` binary on a copy of the tree. The committed tree passes. A slice named twice, a dropped slice and a second cut are each refused by name.
