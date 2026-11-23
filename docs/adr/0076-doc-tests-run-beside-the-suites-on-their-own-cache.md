# ADR 0076: doc tests run beside the suites, on their own cache

Status: **accepted**, 2026-11-23.

Stakeholders: the Director; a pull request author; the required "Build and workspace tests" row.
Concerns: the 300 s run wall holds with margin on every run, and every doc test still runs (#1095).
Requirements: INV-CI-DOC-PATH-1.
Viewpoint: the `ci.yml` job graph, as `check-gate-parity` reads it.
Elements and relations: `ci.yml` jobs `workspace_tail`, `doc_tests`, `debug_e2e`, `build_verdict`.

## Context

`Doc tests` was the last step of the evidence job. It ran after the journey, about 200 s, so the
end-to-end row and the required row waited on it. Runs walled at 286 to 317 s with every test green
(#1092 to #1094; #1125, run 45086616792, 313 s). Under the shared `workspace` cache key, the first job
on main to save owned the entry with a `dev` build. So every run compiled 249 crates again under the
test profile: 3 m 19 s on an exact hit (run 45056931513).

## Decision

1. **A `doc_tests` job of its own.** It starts with the suites, and only `build_verdict` needs it.
2. **Its own cache key, `doc-tests`.** Main saves its test-profile build, and a pull request restores it.
3. **Rule 11 of `check-gate-parity`.** It refuses a job other than `build_verdict` that waits on the
   doc-test job, a doc-test job with no cache key, and a key another job uses.

## Alternatives rejected

- **Own cache key, step kept in the evidence job.** The journey row still waits on the restore and the
  doc build, behind the receipts build, on the one critical path.
- **Raise the 300 s limit, or drop doc tests.** #1095 refuses both.

## Consequences

- The evidence job ends after the journey. Its row is now "Workspace evidence".
- One more cache entry on main. `cache-budget.yml` prunes by last use within 9 GiB (ADR 0041).

## Correspondence

Refines ADR 0041 and ADR 0074. Closes #1095.

## Verification

`the_doc_tests_hold_no_job_but_the_verdict` drives the shipped binary. It failed on the old graph
(exit 0 where the journey waits on the doc tests), and it refuses each case by name on this tree.
