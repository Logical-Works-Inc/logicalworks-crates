# ADR 0029: the logical_ci verdict runs in this repository

Status: **accepted**, 2026-11-12. The Director's order was "I DO NOT FUCKING UNDERSTAND WHY YOU
MENTION BLOCKERS YOU CAN JUST FUCKING FIX."

Stakeholders: the Director; a contributor whose pull request waits on the `logical_ci` check; logical_ci's owners.
Concerns: a required verdict that never ran; a private product that must stay private.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the release bytes it executes.
Elements and relations: `.github/workflows/logical_ci.yml`, the signed logical_ci v0.4.1 release, the `LOGICAL_CI_TOKEN` repository secret.

## Context

`logical_ci.yml` called `Logical-Works-Inc/logical_ci/.github/workflows/gate.yml@v0.1.0`. This
repository is public and logical_ci is private. GitHub never lets a public repository call a
private reusable workflow, so every run since #865 failed in 0 s with no job started. The call
also passed no `LOGICAL_CI_TOKEN`, which gate.yml requires to download the release.

## Decision

1. **The gate's steps run here.** `logical_ci.yml` carries logical_ci's gate.yml at
   `d0daddd2` on its macos-arm64 asset, pinned to the signed logical_ci v0.4.1 release, and
   installs cargo-nextest 0.9.149 unless the runner holds it (first `75a0a1f5` and v0.2.0, then
   `05cc9689` and v0.3.0). It keeps that file's release key pin,
   its verifier sha256 pin and its order: refuse an unpinned gate, download, check the verifier,
   verify the seal, then `plan --new`, `init` and `ci` for tenant `crates` at hardness C.
2. **A missing token is named.** The download step refuses with `LOGICAL_CI_TOKEN is not set on
   this repository` before it calls `gh`.
3. **A fork's pull request skips the job.** GitHub hands a fork no secret, so the job could only
   fail at the download.
4. **No workflow calls another repository's.** `check-gate-parity` gains rule 7 and now holds
   every workflow, not only `ci.yml` and `pages.yml`, to rules 5 and 7.

## Alternatives rejected

- **Making logical_ci public.** It is a private product, and every other product depends on its
  code staying private.
- **A self-hosted runner.** Its runner would run a public repository's code on the Director's Mac.
- **Removing the job.** The verdict is the one the Director ordered in #802.

## Consequences

- The inline copy drifts when logical_ci re-pins. The header names the commit it copies, and a
  re-pin changes `LCI_RELEASE` and both pins here in one commit.
- The job runs on `macos-latest`: v0.4.1 ships a linux-arm64 pair, but Linux refuses `ci` until
  logical_ci#85 gives lanes containment.
- v0.3.0's loom run built every member in release, though lgwks_std alone models loom. A02
  sealed UNMEASURED when its lane ended at 397.9 s (run 43244596498). v0.4.1 builds only the
  modelling packages, and on this tree its property-and-loom gate holds in 82.42 s, 2.46 GB peak.
- With v0.4.1 the lane still ended at 397.1 s (run 43273499037): the engine compiled into its
  own cache directory, which no rust-cache entry holds. `LOGICAL_CI_TARGET_DIR` puts its builds
  in the workspace target that hosted-rust restores, and `show` prints each lane's output.
- `show` named the cost: cold, `cargo nextest run` was killed at 397.8 s (run 43278464658),
  while the 26 property tests take 1.6 s and a built target runs the lane in 43.4 s. The job
  builds the lane's test targets with the same command first, so the lane runs on them.
- v0.4.1's seal names all four binaries, and `lci-verify` exits 2 with the macOS pair alone, so
  the download takes every `logical_ci-*` and `lci-verify-*` asset (13.7 MB).
- The engine compiles this repository, so the job first runs `.github/actions/hosted-rust`:
  on a bare runner `.cargo/config.toml`'s ld64.lld link failed every compile in 30 s and A02
  sealed NO-GO (run 43242590489).
- The policy gates A02, and `ci/property-and-loom` probes cargo-nextest first. Without it the hosted
  run sealed UNMEASURED in 142 ms (run 43232168262). With nextest 0.9.149, `ci` on 0531bb6
  returns GO with A02 passed, in 153.5 s wall and 2.35 GB peak RSS on the logical_ci Mac.

## Correspondence

Refines #802 and #865, which first called the gate. Relates to ADR 0028, the budget the job's
10-minute cap sits under.

## Verification

- The `gate-parity` lane's rule 7 refuses a `uses:` of another repository's reusable workflow
  in any workflow, by its line: `a_call_to_another_repositorys_reusable_workflow_is_refused`
  and `every_workflow_the_checker_reads_is_held_to_rule_7`.
- The `logical_ci` job on every push to main and every same-repository pull request. A GO is a
  `logical_ci GO` line; anything else fails the job and names NO-GO or UNMEASURED.
- `Pin the verifier` fails on a swapped verifier, and `Verify release plan and seal` fails on a
  swapped release.
