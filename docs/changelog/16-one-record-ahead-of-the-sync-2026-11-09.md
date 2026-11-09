# lgwks_ast 6.0.2 / lgwks_deps 6.3.0 / lgwks_macros 2.1.1 / lgwks_bot 8.2.0 / drishti 0.1.4

Released 2026-11-09. A view can show a record before its sync lands, the dependency command lists the capability catalog, and every page was audited against the tree.

## lgwks_bot 8.2.0 (minor)

- `RecordProjection::empty` starts a view with no journal, and `fold_one` folds one entry under the rules a catch-up has (#897, ADR 0024). A refused record poisons the view either way.
- `RecordEntry::staged` makes the entry for a record a consumer has written but not synced. Its head is a constant, so a staged entry is not evidence of the chain.
- A view fed ahead of its file can show a record a crash loses. A restart rebuilds from the file, which stays the truth.
- The `record-projection` binary gains `stage` and `sync`, and holds at most 4,096 staged records.

## lgwks_deps 6.3.0 (minor)

- `lgwks-deps catalog [ROOT] [--json]` lists every capability the estate owns, its owner and whether it is on by default.
- `check-docs` reads a line as a heading only when CommonMark 0.31 does, so a wrapped `#108)` stays prose.
- `check-std-first` reads the disk when git lists nothing, which is what git answers in a copy under another checkout's ignored directory.
- `check-doc-citations` names `docs/bevy-admission-hazard.md` as a page citing the Bevy repository.

## lgwks_ast 6.0.2, lgwks_macros 2.1.1, drishti 0.1.4 (patch)

These carry documentation corrections from the audit and change no code. `lgwks_std` did not move and stays 4.1.1.
