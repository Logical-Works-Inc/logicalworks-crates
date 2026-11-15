# ADR 0062: test builds of workspace crates are incremental

Status: **accepted**, 2026-11-15. Found while landing #990; tracked in #1059.

Stakeholders: anyone who commits to a workspace crate; the commit mutation gate; CI.
Concerns: a one-line change that rebuilds a whole crate; a gate that can judge no mutant; CI cache keys.
Requirements: R9
Viewpoint: the build viewpoint, one mutant from the edit to its last test.
Elements and relations: `Cargo.toml` `[profile.test]` and `[profile.ci-it]`, `.cargo/config.toml`, `tests/it/session/limits.rs`.

## Context

The test profile inherited `incremental = false` from the repository's dev profile, which ADR 0027 set for sccache.
The commit mutation gate changes one line, then rebuilds and runs the nearest tests, with at most 180 s per mutant.
In the gate's copy, one `lgwks_bot` mutant rebuilt only `lgwks_bot`, and that took 262 s at opt-level 1. Seven #990 commits were refused with no mutant judged.

## Decision

1. `[profile.test]` sets `incremental = true`. Cargo applies it to workspace crates only, so registry crates keep their sccache entries.
2. `[profile.ci-it]` sets `incremental = false`. CI jobs build cold and have nothing to reuse, and `ci-bot` inherits it.
3. Dev, check and clippy keep `incremental = false` from ADR 0027.

## Alternatives rejected

- **Lower `lgwks_bot` to opt-level 0 in tests.** The stack-bound regression is calibrated at opt-level 1, and the sims run slower.
- **Split `lgwks_bot`.** That is the right long-run fix for its size, but it is a refactor of its own and does not help any other crate.

## Consequences

- Measured on the #990 tree, same command, same mutant (`crates/lgwks-bot/src/session/runner/record.rs:198`, `>` to `>=`): 281 s before, 37 s after. That is about 12 s to build, 8 s of setup and 17 s for 174 tests.
- Each test target dir keeps an incremental cache for workspace crates. Its size counts toward the disk budget.

## Correspondence

Refines ADR 0027 for the test profile only: dev, check and clippy keep its `incremental = false`. Relates to ADR 0028, which sets the cold-run budget CI keeps.

## Verification

- `cargo nextest run --locked -p lgwks_bot --all-features --test it -E 'test(~record)'` after a one-line change rebuilds in seconds.
- `an_ask_prompt_exactly_at_the_record_ceiling_is_recorded_and_one_byte_over_is_refused` fails on that mutant and passes on the code.
