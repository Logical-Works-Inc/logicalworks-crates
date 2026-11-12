# ADR 0035: a gate reads what ships

Status: **accepted**, 2026-11-12. Issue #911 items 4 and 9 named the gates that an author could
pass without doing the right thing, from the architecture review of 2026-11-10.

Stakeholders: the Director, who reads a green required check as the work being right; an agent
authoring a pull request with no human on it.
Concerns: a lane that cannot fail, and source that ships but no gate reads.
Requirements: R13
Viewpoint: the gate viewpoint, one element per gate binary and the text it reads.
Elements and relations: `check-gate-parity`, whose rule 7 lives in `check_gate_parity::neuter`;
`check-std-first` and the `ci-local` unwrap lane, which share the lexer in
`src/bin/shared/rust_code.rs`; and `lgwks-ast scan`, whose walk decides what it skips.

## Context

Parity rewrote `||` to `&&` before comparing a step with its lane, so `cargo clippy ... || true`
agreed with the clippy lane. It never read `if: false` or `continue-on-error: true`. The unwrap
lane exempted every line after the first that contained `mod tests`, a comment included. The scan
skipped `vendor` at any depth, so `crates/lgwks-bot/src/vendor/` would ship unscanned. The FFI
pattern wanted a word after `"C"` and missed `extern "C" {`.

## Decision

Refuse a gate step whose failure is swallowed, judging a `||` operand by the command whose status
it leaves. Refuse an `if:` that is always false and `continue-on-error: true` on any job or step.
Exempt from the unwrap lane only the lines a `cfg(test)` item spans, read through one lexer.
Skip a directory by name only where that name is what it says.

## Alternatives rejected

- **Stop rewriting `||` in rule 2.** Every lane whose step has a real `|| { ..; exit 1; }` report
  would then disagree with its command, and the escape would still pass a looser comparison.
- **Parse the workflow with a YAML crate.** It is a new dependency for one reader. The line reader
  already keys on the indentation GitHub's own schema fixes.
- **Keep the skip names and add a list of shipped `vendor` paths.** A list nobody updates is the
  blind spot again, one directory later.

## Consequences

- A gate step can no longer end `|| echo`; a setup step the gate does not claim still may, as
  `Cache hit rate` does.
- Rule 7 reads `ci.yml` only; `pages.yml` runs no lane.
- On this tree, `check-gate-parity`, the `std-first` lane and the `unwrap-scan` lane all pass, so
  nothing shipped relied on an escape.

## Correspondence

Refines ADR 0028's rule 6 reader, which rule 7 reuses.

## Verification

- `check_gate_parity::neuter::tests` refuses every escape above and keeps the controls green
  (INV-CI-NEUTER-1).
- `ci_local::test_gated::tests`, `tests/it/scan_evidence.rs` and
  `check_std_first::scans::tests` hold the scan scopes (INV-SCAN-GATED-1).
