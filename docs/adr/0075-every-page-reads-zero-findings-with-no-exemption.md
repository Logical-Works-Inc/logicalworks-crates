# ADR 0075: every page reads 0 findings, with no exemption

Status: **accepted**, 2026-11-22.

Stakeholders: the Director; a reader of `AGENTS.md`; an agent that starts from it.
Concerns: the Director's order "THERE SHOULD BE 0 LINT FINDINGS", "ALWAYS".
Requirements: INV-DOC-9.
Viewpoint: the documentation set, as the shipped `check-docs` reads it.
Elements and relations: `AGENTS.md`, `INVARIANTS.md`, `tests/it/doc_voice.rs`.

## Context

INV-DOC-9 holds every page to 0 `check-docs` findings. It exempted `AGENTS.md`, because the contract hook
sealed that file until the Director named it. Three findings stayed in it: an em dash at line 14, and
sentences of 63 and 46 words at lines 102 and 108. The Director named the file on 2026-11-22.

## Decision

1. **Fix the three lines in place.** No line is added or moved, so every pinned citation resolves.
2. **Drop the exemption.** `the_whole_tree_reads_zero_findings` requires exit 0 and an empty stderr.

## Alternatives rejected

- **Keep the exemption.** It had one cause, the seal, and the seal is gone.
- **A second whole-tree test.** That is two implementations of one check.

## Consequences

- A finding on any page, `AGENTS.md` included, fails the suite.
- A crash of `check-docs` fails too, since it writes to stderr or exits non-zero.

## Correspondence

Refines INV-DOC-9. Relates to ADR 0011 and ADR 0023.

## Verification

`the_whole_tree_reads_zero_findings` fails on the old line 14, naming `AGENTS.md:14`, and passes on this
tree: 75 records, 378 pages.
