# Workflow §1, §10, §11: local CI, the runner map, the WWFD cycle

Moved from `WORKFLOW.md` on 2026-10-22, when every Markdown file in a
repository was held to 120 lines (Director, 2026-10-10) and that file had grown
to 445. `WORKFLOW.md` is the index and keeps the § numbers.

---

## 1. Local CI

Local CI is the gate. It runs every check a pull request needs, on this machine,
without GitHub, and writes a receipt.

```sh
./scripts/ci-local.sh              # full gate
./scripts/ci-local.sh --fast       # the lanes marked fast
./scripts/ci-local.sh --lane <id>    # one lane, what CI runs per step
./scripts/ci-local.sh --list       # lane ids and surfaces
./scripts/ci-local.sh --receipt    # also write evidence/ci-local-<sha>.md
```
`scripts/gate-lanes.toml` is the one definition of "the gate". The `ci-local`
binary of `lgwks_deps` executes the lanes from it; `.github/workflows/ci.yml` runs
the same commands against the same lane ids; the `check-gate-parity` binary refuses
any drift between the two. They do **not** run in the same order — CI fans the lanes out
across jobs and needs — and a green local run is evidence only for the lanes
whose `surfaces` include `local`. The receipt names every lane this surface
did not cover.

When the two disagree, the lane table is the source of truth and the
disagreement is a defect in whichever side diverged. Fix that side and re-run
both.

Two evidence lanes exist because they are user-facing claims rather than
ordinary unit-test counts. `simulation-evidence` parses Nextest's executable
test listing and also counts source-visible `#[test]` attributes under
`tests/sim/` and `sim_*` files, then refuses the gate unless each view is at
least half deterministic simulation. `debug-e2e` drives the public
`lgwks-deps debug` command through its JSON success path and a fail-closed
manifest fixture, and prints the successful end-to-end journey result CI must
show. CI runs both lanes in the workspace job and exposes each verdict as its
own PR check row, `successful simulation result` and `successful end-to-end
journey result`, so the checks list shows the result without requiring a reader
to open the aggregate log. A check row reads the report its lane wrote and
passes only on that lane's own pass line; it does not build the workspace again.

Every job runs on a GitHub-hosted runner, because the repository is public and a
self-hosted runner would execute a fork's pull request on its host
(INV-CI-RUNNER-CLASS-1). The specification of CI is §12.

The `check-gate-parity` binary refuses a required lane with no matching CI step, a CI gate step no lane claims, a
substituted command, a toolchain pin that drifted, and a job naming `self-hosted`.

---

## 10. Local CI and the runner map

| Repo | Gate script | Runner | Workflow |
|---|---|---|---|
| `logicalworks-crates` | `scripts/ci-local.sh` | GitHub-hosted runners only: Linux, macOS and Windows | `.github/workflows/ci.yml`, specified by §12 |

Keep the OS matrix: dropping a leg is a coverage loss, not a speed-up.

Linux is the correct rung wherever a test is `#![cfg(target_os = "linux")]`. On a
non-Linux host such a test compiles to nothing and the suite reports green having
verified nothing, which is silent coverage loss. macOS is a reduced rung for quick
signal, not an equivalent one.

---

## 11. Mandatory WWFD cycle

**Before implementation**, query WWFD (`wwfd q "<error or concept>"` or MCP
`wwfd_query`). Check whether the problem or architecture has past lessons or
established estate standards. Never invent from scratch what WWFD already
solved.

**After verification and before delivery**, run
`wwfd state sync <repo> --task '<task>'` to record genuine state before closing
or opening a PR. No claim of green completion without a sync receipt.
