# ADR 0043: an advisory match carries its title and a skipped pin its name

Status: **accepted**, 2026-11-13, for #955. logical_ci#250 deletes its own RustSec reader once
these are released.

Stakeholders: logical_ci, whose refused, waived and informational rows each carry a title and
whose REQ-ADV-001 journey names every pin not from crates.io; every operator of `lgwks-deps
advisories --json`.
Concerns: an untitled row a reader cannot triage; a skipped pin counted but never named.
Requirements: INV-SEC-AUDIT-3
Viewpoint: the gate viewpoint, one report per audit.
Elements and relations: `advisories::parse` (the title), `advisories::db` (the skipped pins),
`advisories::types` (`AdvisoryMatch`, `SkippedPin`, `AdvisoryReport`) and `deps_cmd::advisories`.

## Context

advisory-db's format v3 writes an advisory's title as the first `# ` heading after the TOML
front matter, and no `title` key. The parser read only the key, so every live advisory was
untitled. The report counted the pins it skipped but did not say which pins they were.

## Decision

- A `title` key, when a file writes one, is the title. Otherwise the title is the first line
  after the front matter when it is an `# ` heading with text after it. Blank lines and CRLF
  line endings before it are skipped.
- `AdvisoryMatch::title` returns it. `AdvisoryReport::skipped` returns every skipped pin by name,
  version, lockfile and source. The list is sorted, holds each pin once, and is bounded together
  with the audited pins by the audit's package bound.
- `lgwks-deps advisories --json` writes `title` on each match and a `skipped` array.

## Alternatives rejected

- **Make `is_crates_io` public and let each caller rerun it over `lock::parse`.** That puts a
  second copy of the source rule in every caller, and the copies drift from the audit.
- **Keep reading the `title` key.** advisory-db does not write it.

## Consequences

- Adding these is a minor change to lgwks_deps. `packages_skipped` is now the length of
  `skipped`, so the count and the list cannot disagree.

## Correspondence

Refines ADR 0038 and INV-SEC-AUDIT-2. Indexed in CODEBOOK.md's `advisories` row.

## Verification

- `sim_advisories_title_is_the_first_heading` sweeps the openings a body can have against a
  reference model.
- `sim_advisories_reports_every_row_cargo_audit_reports` holds each fixture match to its title
  and the four skipped pins to their names.
- `sim_advisories_only_registry_sources_match` holds the list to its count and its order.
