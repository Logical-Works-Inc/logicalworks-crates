# ADR 0041: the cache keeps the builds that set the wall

Status: **accepted**, 2026-11-13. The Director asked "Why is cache not auto setting to stay
under 10? Is this ur definition of fixing cache?" after ADR 0037 left main over the cap.

Stakeholders: the Director; every pull request whose jobs restore main's cache.
Concerns: GitHub's 10 GB repository cache cap, and which entries its eviction drops.
Requirements: R13
Viewpoint: the gate viewpoint, one row per CI job and the cache entry it saves.
Elements and relations: the `hosted-rust` action's `save-cache` input, the jobs of `ci.yml` and
`pages.yml` that pass it, and `.github/workflows/cache-budget.yml`.

## Context

After ADR 0037, main held 50 entries, 15.8 GB. Six were a set the moved lockfile replaced
(2.4 GB), and the other 44 were current, 13.4 GB on their own. Past the cap GitHub evicts by
last use, so every main run evicted a current entry, and the job that lost it saved again on
the next run. The clippy, timing and feature lanes logged "No cache found" on three main runs
in a row (runs 43345895098, 43397802234, 43399257060).

Cold, on run 43397802234, those jobs finish within 150 s, half the 300 s wall. The macOS and
Linux test shards, the Windows jobs and the storefront legs run 155 to 292 s cold. Only the
second group sets a cold run's wall.

## Decision

- A job saves its dependency build from main only when it compiles longer than 150 s cold.
  `hosted-rust` takes `save-cache`, and gate, no-default, clippy, docs, the target matrix,
  timing, the three lgwks-bot feature parts, Linux AppCUI, storefront deps and Pages pass
  false. Those entries were 5.2 GB.
- `cache-budget.yml` runs after every main run of `ci` and `logical_ci`, and daily. It
  deletes each pull request's entry and each main entry not touched since the earliest start
  of the saving workflows' latest successful main runs. When those runs share one commit and
  the set still passes 9 GiB, the job fails and lists the largest entries.

## Alternatives rejected

- **A larger cap.** It is billed (the API returns HTTP 402 without a payment method), and the
  churn returns with the next growth.
- **One shared registry entry.** Each entry's crate files are 10 to 152 MB by lane, about 2 GB
  in all (estimated from `cargo tree` against the local registry). rust-cache cannot leave the
  registry out, and the current set would still pass 10 GB.
- **Pruning by age.** A hit never re-saves, so the oldest entry is often the one main restores.

## Consequences

- The current set drops to about 8 GB (estimated from the entry sizes), so GitHub never has to
  evict, and the long jobs stay warm.
- The jobs that save nothing compile cold on every run, as they already did under eviction.
- The 5.2 GB of entries those jobs saved before this are deleted by hand once, at the merge,
  since main's runs still restore them.

## Correspondence

Supersedes the size consequence of ADR 0037. ADR 0036's prefix restore finds main's newest
entry for a group, which the pruner keeps. Described in `docs/workflow/ci-layout.md`.

## Verification

- `check_gate_parity::cache_budget::the_cache_holds_only_the_builds_that_set_the_wall` holds
  the committed workflows and names a job that saves against its row, a saving workflow the
  pruner does not follow, pruning by anything but last use, and a missing budget exit.
- `gh api repos/Logical-Works-Inc/logicalworks-crates/actions/cache/usage` reads under 9 GiB
  after the first main run and its `cache-budget` run.
