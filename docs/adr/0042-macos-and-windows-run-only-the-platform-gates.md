# ADR 0042: macOS and Windows run only the platform gates

Status: **accepted**, 2026-11-13. The Director asked "Why is it so macOS heavy? Why are you
not running testing on whatever the cheapest solution is and running Mac + windows running for
ubiquitous gates?"

Stakeholders: the Director; every pull request that waits on hosted macOS runners.
Concerns: GitHub's cap of 50 hosted macOS jobs at once, and the 300 s wall that counts queue.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the runner it takes.
Elements and relations: `tests_macos` and `tests_linux` in `ci.yml`, the receipt step of
`build_verdict`, `docs/acceptance/t-rows-darwin.toml` and `scripts/it_group_rustc.rs`.

## Context

Each crates run started 27 macOS jobs. Three runs overlapped at 01:34 on 2026-11-13, macOS jobs
queued up to 127 s, and #943 failed the 300 s wall with every job green (run 43345936756).
Two runs already pass the cap of 50.

Ten of the 27 ran `sim_review_path` in tenths, for the 100 and 1,000 saturation tiers no receipt
names; Linux runs the other 44 tests of that binary. Ten more ran all 299 tests the Darwin
receipt named, 482 s of tests (run 43428368087), while the Windows job runs only the row-named
`_tNN` tests against its own map.

## Decision

- Every suite runs on `ubuntu-24.04-arm`, the cheapest hosted runner. The Linux saturation row
  runs all three tiers; it ran the 10,000 tier in 62 s.
- macOS runs only the platform gates: lgwks_std's Darwin seams, lgwks_bot's process binaries,
  the Metal, GPUI and AppCUI legs, and the 120 `_tNN` tests (104 s) the Darwin receipt now
  reads from `t-rows-darwin.toml`. Four rows build two `it` groups each, since the group wrapper
  takes a comma list.
- That is 11 macOS jobs a run, so four runs fit under the cap.

## Alternatives rejected

- **Larger macOS rows.** Fewer jobs, but the whole `it` binary did not compile within the 5-minute
  limit on 3 vCPU (run 41700839783).
- **Self-hosted Macs.** The repository is public, so a fork's pull request would run on the
  Director's Mac (province#9).
- **One run at a time.** Agreeing slots between sessions stops the queue but leaves every push
  waiting on the others.

## Consequences

- The 179 other tests `t-rows.toml` names are receipted on Linux alone; each Darwin row's gap
  says how many.
- Each Darwin row compiles two groups, so a cold row is longer than a one-group row was
  (estimated, not yet measured).

## Correspondence

Extends ADR 0037's group wrapper. `t-rows-darwin.toml` follows `t-rows-windows.toml`. Described
in `docs/workflow/ci-layout.md`.

## Verification

- `check_gate_parity::platform_gates::the_platform_rows_run_only_the_platform_gates` holds the
  macOS job count of one run to 25, the Darwin map to every row's `_tNN` tests with its gap
  counts, the Darwin receipt to that map, and the Linux saturation row to every tier.
- The pull request's own run passes its Darwin, Linux and Windows receipts.
