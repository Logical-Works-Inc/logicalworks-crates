# ADR 0033: the bot shards hash optimized

Status: **accepted**, 2026-11-12. The Director's orders were "WHY THE FUCK R UR RUNS TAKING SO
LONG" and "sub 5 minute CI".

Stakeholders: the Director; a contributor whose pull request touches `lgwks_bot`.
Concerns: a run that passes every test and still fails its 300 s wall.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the profile it builds.
Elements and relations: the `ci-bot` profile in `Cargo.toml`, the Linux bot shards that build
with it, and `lgwks_bot`'s journal frame search, which hashes through `lgwks_std::hash`.

## Context

Run 43237164800 (#931) walled 330 s against 300 with every test green. Its last job was bot-rest
slice 2. That slice's 52.8 s test step held `journal::frame::tests::noise_and_the_longest_possible_tail_hold_nothing`
for 40.8 s. The noise case tries every candidate length up to the 64 KiB ceiling and hashes
about 2 GiB through blake3. `ci-bot` builds every dependency at opt-level 0, blake3 included.

## Decision

`[profile.ci-bot.package.blake3]` sets opt-level 3. Every other `ci-bot` dependency stays at
opt-level 0, so the shards' build cost is unchanged apart from that one crate.

## Alternatives rejected

- **A constant head function in the noise case.** The same loop would run without hashing, but
  the test would stop exercising the store's own head on the longest tail, and every other
  hashing test on the shards would stay slow.
- **A smaller ceiling in the test.** It stops covering the 64 KiB ceiling the test is named for.

## Consequences

- Measured locally under `--cargo-profile ci-bot`: the noise test 18.704 s before, 1.024 s after;
  the rebuild compiled blake3 and relinked.
- Measured on CI, Linux bot-rest slice 2: the noise test 40.8 s on run 43237164800, 2.151 s on run
  43243826845; the slice's nextest summary 19.042 s for 400 tests, against a 52.8 s step before.
- The profile tables are part of the shards' cache key (ADR 0028), so the first run after this
  lands builds the bot shards cold, once.

## Correspondence

Refines the `ci-bot` profile ADR 0028's cache layer keys on.

## Verification

- `sim_lane_split::the_bot_shards_compile_blake3_optimized` refuses a manifest whose `ci-bot`
  profile drops the override or changes its level.
- The `Build and workspace tests` row's run-wall step fails a run past 300 s.
