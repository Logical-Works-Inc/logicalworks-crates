# ADR 0078: every test is judged by what it leaves

Status: **accepted**, 2026-11-23.

Stakeholders: the Director; every repository that runs its tests through this workspace's nextest config.
Concerns: no test leaves a file in temp or a live process behind, and none passes while it does.
Requirements: INV-CI-RESIDUE-1, #1128 G2.
Viewpoint: one test process, from the moment nextest starts it to its exit.
Elements and relations: `churn-slot` (`src/bin/churn_slot.rs`, `churn_slot/residue.rs`), `.config/nextest.toml`.

## Context

On 2026-11-23 the build Mac held 110,965 leaked temp files, 86 GiB. Every test that made them passed.
A test is judged by its assertions, and none of them looks at what the process leaves behind.
ADR 0077 fixed the cause found. It cannot show that no other test leaks, and a leak from a tool
the code runs (`xcrun`) is outside any model of the code.

## Decision

1. **A census per test.** The `churn-slot` wrapper gives each test a fresh `TMPDIR`,
   `lwc-test-<16 hex>` under the one it would have used. When the test exits, each entry still
   there fails it, and the first eight are named. The census then removes the directory.
2. **Fail closed.** A census step the host refuses fails the test. A missing base dir is not
   run, exit 125.
3. **Every Unix host.** The wrapper runs on Linux and macOS. Linux runs every suite
   (INV-CI-PLATFORM-1).
4. **Processes are #1128 G2b.** A first cut made the wrapper the Linux child subreaper and
   killed live orphans after the test. It did not reap while the test ran, so a child that
   exited stayed a zombie where init would have reaped it, and 12 tests that probe exit
   failed for the census's own reason. The next cut reaps during the run, as init does.
5. **A served flow drains on a stop signal.** The first census run failed 58 of 8,454 tests.
   One was a product leak: an app served by the `serve` word took the default action on
   `SIGTERM`, so no drain ran and its ephemeral run store stayed for ever. `serve_at` now arms
   `SIGTERM` and `SIGINT` before it binds, and either one drains the serve.

## Alternatives rejected

- **A proof of no leak, seL4 style (Isabelle, Kani).** It proves a model of our code. The leak
  came from `xcrun`, which no model holds. A proof also costs weeks per module.
- **A sweep after the suite.** It finds the leak, but not the test that made it.
- **nextest's `leak-timeout` alone.** It sees only a child that holds the test's stdout or
  stderr. A child that redirects them, or a file, goes unseen.

## Consequences

- A test that leaves residue fails, so each must remove what it makes.
- A file written outside `TMPDIR` (a fixed path) is not seen. That is not claimed.
- Cost: one `create_dir` and one directory walk per test.

## Correspondence

Closes #1128 G2. It extends ADR 0077 from the children `lgwks_bot` starts to every test.

## Verification

`sim_residue_census_matches_the_model` checks the count and names over 256 drawn trees.
`the_census_fails_a_program_that_leaves_residue` runs the real wrapper over 16 seeds of files
and exit codes. It failed before the change.
`new_check_expand_run_serves_one_request` kills `lgwks run` and requires the app's `TMPDIR`
empty. It failed before the serve change.
