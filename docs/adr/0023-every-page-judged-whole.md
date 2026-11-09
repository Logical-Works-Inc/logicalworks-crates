# ADR 0023: every page judged whole

Status: **accepted**, 2026-11-09.

Stakeholders: the Director; every agent that opens this repository; a consumer reading a page on crates.io.
Concerns: an audit by pattern misses what a read finds, pages over the size law cannot be edited at all, and a command a page names may not run.
Requirements: R13
Viewpoint: the whole-documentation viewpoint: every living page, read against the tree it describes.
Elements and relations: the living pages; `docs/requirements/*.md`, which the `check-requirements` binary reads with `REQUIREMENTS.md`; the `catalog` command of `lgwks-deps`; `tests/it/agent_start_page.rs`, which holds them.

## Context

The Director, 2026-11-09: "SO FINISH THE WORK. WHY R U DOING HALF AUDITS AND SHIT". ADR 0022 held the start pages. Reading the rest found closed issues reported open, a security statement the code had overtaken, and commands no binary answers. The catalog itself, a PRD and the estate's contract all named `lgwks-deps catalog`, and it answered "unknown command".

Twenty of the pages were over the 120-line law, which refuses any commit that stages one. A page the law refuses goes stale, because nobody can fix it. `REQUIREMENTS.md` was one of them, and its gate read one file.

## Decision

1. **A page over the law is split at its sections.** The head keeps the path, the title and a line naming each part, so a citation of the page still opens it. A part is `<page>-<topic>.md`.
2. **The requirement spine may span pages.** The `check-requirements` binary reads `REQUIREMENTS.md` and then `docs/requirements/*.md` in name order. An id declared twice is refused, because the later copy would hide an edit to the earlier one. More than 64 pages is refused.
3. **`lgwks-deps catalog` is a command.** It shares one implementation with the `catalog` binary, and the lane keeps running the binary.
4. **A test reads every living page.** Each path, link and `lgwks-deps` command must resolve, and each page keeps to 120 lines. An allow-list names what is another repository's or a recorded input, with the reason.
5. **A heading is one to six `#` and then a space or the end of the line.** The `check-docs` binary read a wrapped `#108)` as a heading (CommonMark 0.31 §4.2).

## Alternatives rejected

- **Exempting long pages from the law.** The law exists so a page stays editable, and no gate reads a reason.
- **Removing the `lgwks-deps catalog` spelling from every page.** Six surfaces and the estate contract name it, and the binary's own doc comment does. Making it true is one module.
- **Reading only the pattern scan's hits.** The closed-issue claims and the stale security statement were prose no pattern named.

## Consequences

- One page's sections now span files, so an anchor link crosses pages, and a section number refers to the head's title rather than to one file.
- The allow-list is a place an error can hide. It holds whole pages only where the page's subject is another repository, and a test refuses a row the page no longer needs.
- A test reads named things, so a false sentence still passes it. Only another read finds the next one.

## Correspondence

Refines ADR 0022, which holds the start pages to the tree, and ADR 0011, which holds a page's shape. INV-DOC-7 and INV-GOV-2 state the rules.

## Verification

`crates/lgwks-deps/tests/it/agent_start_page.rs` holds decisions 1, 3 and 4 on the live tree. `crates/lgwks-deps/tests/it/sim_agent_start_page.rs` plants a command the binary lacks, `crates/lgwks-deps/tests/it/sim_requirements_pages.rs` and `requirements::tests` hold decision 2, and `check_docs::prose::tests` holds decision 5.
