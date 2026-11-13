# lgwks_deps 7.2.0

Released 2026-11-13. An advisory match carries its title, and the report names every pin it skipped.

## lgwks_deps 7.2.0 (minor)

- `AdvisoryMatch::title()` returns a `title` key when a file writes one, else the first `# ` heading after the front matter. advisory-db writes the title only as that heading, so 7.1.0 titled every live advisory `None` (#955, ADR 0043).
- `AdvisoryReport::skipped()` names every pin not from crates.io by name, version, lockfile and source, sorted, and bounded with the audited pins. `packages_skipped()` is its length.
- `lgwks-deps advisories --json` writes `title` on each match and a `skipped` array.

`lgwks_std`, `lgwks_ast`, `lgwks_macros`, `lgwks_bot` and `drishti` did not move.
