# ADR 0037: advisories match as Cargo reads a requirement

Status: **accepted**, 2026-11-12. The Director's order, relayed by logical_ci's session for #942,
was "fix this shit and ensure it never fucking happens again".

Stakeholders: the Director; logical_ci, whose advisory gate calls `lgwks_deps::advisories::audit`
(logical_ci#250); every operator of `lgwks-deps advisories`.
Concerns: a vulnerable pin reported as patched; a database that could not be read reported clean.
Requirements: INV-SEC-AUDIT-2
Viewpoint: the gate viewpoint, one verdict per crates.io pin and advisory.
Elements and relations: `advisories::semver` (the requirement reader), `advisories::db` (layout,
bounds, matching, identity), `crate::git` (the one git launcher), and the `semver` dev oracle.

## Context

lgwks_deps 7.0.0 compared versions without Cargo's pre-release gate, so `patched = [">= 1.2.3"]`
read `2.0.0-alpha.1` as patched: a fail-open in a security audit. It also had no library entry,
so logical_ci kept a second RustSec reader of its own. Running cargo-audit 0.22.2 over a fixture
database showed that cargo-audit does not apply the gate either, and that it does not match a
pin whose source is `sparse+https://index.crates.io/`.

## Decision

- Requirements are read by a port of the `semver` 1.0.28 parser and matcher, so a pre-release
  satisfies a requirement only through a comparator naming its own `major.minor.patch`. A
  requirement RustSec refuses to load (a wildcard, more than one lower or upper bound, `^`, `~`
  or `=` beside another comparator, more than two comparators) is a typed refusal.
- Only crates.io pins are matched, in both spellings. Every other pin is counted as skipped.
- The report carries each match's lockfile and `[affected]` os and arch, and names the
  database by its `HEAD` commit and date and a digest of the files read.
- The estate reports a superset of cargo-audit's rows. On the fixture the extras are exactly
  the gated pre-release and the sparse pin, both fail-closed.

## Alternatives rejected

- **Depend on `semver` or `rustsec` at runtime.** `rustsec` brings a git client and a network
  fetcher into an offline audit. `semver` would be a second version model beside the lock
  reader's. It is used as a test oracle only, a dev edge already in `Cargo.lock`.
- **Match cargo-audit exactly.** That keeps the fail-open this record exists to close.

## Consequences

- A pre-release pinned below a patched release now fails the audit when it is not patched,
  where cargo-audit stays silent. logical_ci's REQ-ADV-001 tests are the acceptance.
- Waivers, staleness policy and multi-lockfile subject walks stay in logical_ci.

## Correspondence

Refines INV-SEC-AUDIT-1, whose `cargo audit` lane stays the repository's own gate. Indexed in
CODEBOOK.md's `advisories` row.

## Verification

- `sim_advisories_semver_agrees_with_version_req` checks 120,000 generated cases against the
  `semver` crate, and a seed replays to the same trace.
- `sim_advisories_reports_every_row_cargo_audit_reports` holds the recorded cargo-audit rows.
- The other INV-SEC-AUDIT-2 sims cover sources, layout, refusals and bounds.
