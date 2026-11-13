# ADR 0040: the register keeps to the law at 120 columns

Status: **accepted**, 2026-11-13. The Director asked "WHY IS INVARIANTS.MD SO LONG. DOESNT UR GLOBAL RULE BAN
MD FILES LONGER THAN 120 LINES".

Stakeholders: the Director; every agent and contributor who reads the register before the first edit.
Concerns: a register that obeys the 120-line law by newline count while a reader sees hundreds of lines.
Requirements: INV-DOC-8
Viewpoint: the reader's viewpoint, one line as a 120-column terminal wraps it.
Elements and relations: the root `INVARIANTS.md` and each crate's `INVARIANTS.md` and `INVARIANTS-<n>.md`
parts, read by the `invariants` lane through `ci_local::invariants::registers`.

## Context

The register promised one line per rule, and the law counts newlines. Rules grew into paragraphs: the root file
held 102 newlines and 76,842 characters, one line 4,384 characters long, and `lgwks_bot`'s part held 120 newlines
and 86,598 characters. At 120 columns they read as 688 and 780 lines. Each enforced-by clause listed every test
case by name, a second copy of what the test file already says.

## Decision

- Each register part keeps to 120 lines counted at 120 columns. A line of `n` characters counts `n / 120`
  rounded up, and an empty line counts one.
- The `invariants` lane measures every part it reads and refuses one past the law, naming the file and its height.
- Each rule is one short line. Its enforced-by clause names the test file, module or lane that holds it, and the
  test cases stay in that file.
- The rules sit with the crate that owns them, and a crate whose rules need more than one part numbers them
  `INVARIANTS-2.md`, `INVARIANTS-3.md` and on. The lane reads every numbered part.

## Alternatives rejected

- **Hard-wrap the old text at 120 columns.** Wrapping keeps every character, so the register stays 688 lines.
- **Cap each physical line, as MD013 does.** That splits a one-line rule across lines and still says nothing
  about how long the file is.

## Consequences

- The register shrank from 167,817 bytes to 42,448 characters. All 225 rule ids are kept, and INV-DOC-8 is new.
- A reader who wants a rule's cases opens the named test file. The lane still checks every named file, module and
  lane exists.
- Other pages are still held by the newline law; this record binds the register only.

## Correspondence

Refines ADR 0022 and ADR 0023, which hold living pages to 120 lines. Indexed in `docs/adr/README.md`.

## Verification

- `visual_lines_charge_each_line_its_wrapped_height` pins the count: an empty line, one past the width, and
  characters rather than bytes.
- `a_register_past_the_law_is_refused_by_its_wrapped_height` passes a register of exactly 120 lines at 120 columns
  and refuses one character more.
- `a_second_crate_register_file_is_read` proves a numbered part is read and measured.
- `the_honest_register_resolves` runs the lane on the shipped tree.
