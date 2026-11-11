# ADR 0026: tests run on a shared RAM volume

Status: **accepted**, 2026-11-11. Implements #901 for every local run.

Stakeholders: the Director; anyone running `cargo nextest run` on a Mac, the mutation gate included; `lgwks_deps` maintainers.
Concerns: a 90 s test budget; a gate that reruns a crate's suite once per mutant; RAM on a 24 GB host with zero swap.
Requirements: R9
Viewpoint: the test-process viewpoint, mapping a macOS test to the directory its files land in and the slots it shares.
Elements and relations: the `churn-slot` wrapper, the shared `ram_disk` module it and `ci-local` include, and `.config/nextest.toml`.

## Context

The full `lgwks_bot` suite took 671 s on the macOS default `TMPDIR`. Each `sync_all` there is an `F_FULLFSYNC` that queues behind every other. `ci-local` and each CI job already put `TMPDIR` on a RAM disk, but a plain `cargo nextest run` did not. The mutation gate runs one of those per mutant, so a gate took about 40 minutes.

On a RAM disk at six slots the suite took 144 s. One review test took 0.42 s alone and 44.5 s in the suite. The difference was the queue for six slots across fifteen threads.

## Decision

1. **A test on the default directory moves.** The wrapper sends a test whose `TMPDIR` is unset or under `/var/folders/` to `/Volumes/lgwks-test-tmp/tmp/`.
2. **One volume, made once.** The first wrapper to find it absent attaches it under `volume.lock`, and every later one reuses it. It is kept until ejected or the host restarts.
3. **Off the fsync queue, one slot per core.** The six slots stay for a caller's own directory.
4. **One attach routine.** `ci-local` and `churn-slot` include `src/bin/shared/ram_disk.rs`.
5. **The long families start first.** `sim_review_path`, `sim_cas` and `sim_gh_issue` run at priority 50.

## Alternatives rejected

- **A setup script exporting `TMPDIR`.** It could not own a volume past its own exit, and the wrapper already runs per test.
- **Fewer seeds in the slow families.** That shrinks the evidence, and the time was queueing, not work.
- **A volume per run.** A run killed with `kill -9` would leave one behind each time.

## Consequences

- The full `lgwks_bot` suite took 86.1 s and 87.3 s wall on 2026-11-11, with all 5,732 tests passing and a 432 MB peak RSS at a load average of 9.7.
- The volume holds at most 2 GiB, taken as written. One full suite leaves about 5 MiB behind.
- `LWC_CHURN_TMP=inherit` keeps the default directory, and a volume that cannot be made keeps it and says so.
- CI jobs name their own `TMPDIR`, so CI is unchanged.

## Correspondence

Refines INV-CI-SLOT-1 (#272), whose slots this keeps for every caller that names its own directory.

## Verification

- `tests/it/sim_churn_slot.rs` (`only_a_default_temporary_directory_is_moved_to_the_shared_volume`) drives the shipped wrapper.
- The `churn-slot` unit tests `takers_starting_at_once_attach_the_volume_once`, `a_volume_that_cannot_be_attached_is_a_volume_error` and `only_the_default_temporary_directory_moves_to_the_shared_volume`.
