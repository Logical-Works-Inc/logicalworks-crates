# ADR 0090: a lease ledger releases what a dead owner and its lifeline held

Status: **accepted**, 2026-11-27.

Stakeholders: the Director; every process that holds a group, a disk or a release program on the lifeline.
Concerns: the lifeline is SIGKILLed while its owner lives, then the owner, and no process knows the holds.
Requirements: INV-STD-LEDGER-1.
Viewpoint: the process-lifetime view of `lgwks_std::process::lifeline`.
Elements and relations: `lifeline::ledger`, one record file per owner; the lifeline, which takes that record as its
stdout; the sweep in the first hold of every process.

## Context

ADR 0087 ties each hold to a lifeline, a `sh` that releases when its owner's pipe closes. #1160 asks for the case it
leaves: the lifeline dies first (an OOM kill, a stray `kill -9`), then the owner. Nothing that knew the holds is left.
The issue also asks that every later start sweeps what dead owners held, and that the ledger has a bound and a typed
overflow.

## Decision

1. Each hold writes a line to its owner's record before it returns, and `fsync`s it. A let-go hold writes its `-` line.
2. The owner holds an exclusive `flock` on its record for its whole life. The lifeline takes the same open file as its
   stdout, so it holds the same lock. The lock is free only when neither lives. No pid is read to decide that.
3. The first hold of each process sweeps every ledger directory before it opens its own record. A record whose lock it
   takes is dead. Its groups are reaped by `ProcessIdentity`. A device is detached only on the same boot, while
   `hdiutil info` still names the image it was held as, and while no live record holds it. Release programs run last,
   each bounded to 10 s. Then the record is removed.
4. A lifeline that releases removes its own record, so a release never runs twice.
5. The directory is `LGWKS_LEASE_DIR`, else the user state dir (`$XDG_STATE_HOME` or `~/.local/state`, `lgwks/leases`),
   else a per-user temp dir. A hold with no writable directory is refused and names `LGWKS_LEASE_DIR`.
6. A sweep reads at most 4,096 records of at most 4 MiB each. A record past 4,096 lines is rewritten with only its open
   leases, under a fresh lock, and renamed over. The lifeline is then restarted on the new file.

## Alternatives rejected

- **SQLite or redb in `lgwks_std`.** It adds a storage dependency to the core crate, which the dependency doctrine
  forbids. A row also cannot say whether its owner lives. A kernel lock can, and a reused pid cannot fool it.
- **A pid and start time per record, read at sweep time.** The owner's identity says nothing about its lifeline. A
  record whose owner died while its lifeline still releases would be released twice.
- **One shared ledger file.** Every hold of every process would contend on one lock, and one torn write would cost all
  owners their leases.

## Consequences

- Each hold and each release costs one `fsync(2)`. The test `sim_lifeline_record_rewrite_*` makes 4,097 holds and
  releases in 0.149 s with `fsync`, about 36 µs each, and in 13.1 s with `File::sync_data`, which is `F_FULLFSYNC` on
  macOS (Apple M-series, 2026-11-27). So the ledger uses `fsync`. A power cut can lose a lease the drive had not yet
  written, but it also ends every process and RAM disk that lease could name.
- A group whose leader exited before the hold has no identity, so only the lifeline holds it.
- A release program must be safe to run once more after a crash between its run and the record's removal.

## Correspondence

Extends ADR 0087 (the lifeline). Part of #1160 mechanism 3.

## Verification

The test `sim_ledger_sweep_releases_only_what_dead_owners_held` SIGKILLs the lifeline, then its owner, over seeded
plans. A later process's first hold must stop every held group and run every held release. The test
`sim_ledger_sweep_releases_exactly_the_dead_records_leases` sweeps 256 seeded hostile directories against a model.
