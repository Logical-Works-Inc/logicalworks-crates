# ADR 0012: every new public item shows an example

Status: **accepted**, 2026-11-04. The Director's order of 2026-11-04 was "IT SHOULD BE GOOD
ENOUGH THAT A DUMB FUCKING LLM CAN NEXT TOKEN INTO WRITING AND PERFECTING CODE". It follows the
order behind ADR 0011 for Tokio-level docs, with no model call.

Stakeholders: the Director; every caller of a published crate, a person or a small model; a contributor adding an item.
Concerns: a public item whose doc says what it is for and never shows the call, so a reader guesses the arguments.
Requirements: R13
Viewpoint: the gate viewpoint, one rule and the lines of a change it judges.
Elements and relations: `lgwks_ast::scan` with its opt-in `missing-example` rule, `lgwks_ast::hunks`, the `lgwks-ast scan --base` command, `check-docs --base`, and the `examples` lane. The lane runs the command, the command reads the change through `hunks` and keeps the rule's findings on added lines.

## Context

The workspace already denies `missing_docs`, so every public item has prose. Prose is not
enough for a model writing the next line. It needs a call to copy: the argument order, the
`?`, the import path. Tokio shows one on nearly every public item.

The estate does not. Rustdoc's JSON for the 2026-11-04 cut counted examples on 9 of 480 public
items in `lgwks_bot`, and on none in `lgwks_ast`, `lgwks_deps` or `lgwks_macros`. The new rule
counts 3,479 public library items with no example across the six crates: 2,313 in
`lgwks_bot`, 588 in `lgwks_std`, 261 in `lgwks_ast`, 166 in `lgwks_deps`, 150 in `drishti`
and 1 in `lgwks_macros`.

## Decision

1. **`missing-example` is a scan rule.** A plain `pub` item in a library module is a finding when
   its outer doc opens no fenced code block. The kinds are fn, struct, enum, union, trait, type,
   const and static. Test code, `#[doc(hidden)]` items, and files under `bin`, `tests`,
   `examples` or `benches` are not judged, because rustdoc never renders or runs them there.
2. **The rule is opt-in.** `Rule::OPT_IN` holds it, so `scan_source` and the bare `scan` lane
   leave it out. A caller names it with `--rule missing-example`.
3. **The gate judges the lines a change adds.** `lgwks-ast scan --base REV` keeps only findings
   on lines added since `REV`, and a file git does not track yet counts as added whole. A base
   git cannot read is a refusal with exit 2, never a pass. CI passes `HEAD^1`.
4. **One diff reader.** `lgwks_ast::hunks` reads `git diff --unified=0` for this command and for
   `check-docs --base`, so the two gates agree on what a change wrote.

## Alternatives rejected

- **Hold every item now.** 3,479 findings would block every change until all were written, and
  the doc tests would grow past the 5-minute budget in one step.
- **Rustdoc's `missing_doc_code_examples` lint.** It is nightly-only and judges whole crates, so
  it cannot hold just the lines a change adds.
- **A model to write or judge examples.** The Director refused extra model calls, and a model's
  verdict changes between runs.

## Consequences

- Every new public item lands with an example that `cargo test --doc` compiles and runs.
- A bare `lgwks-ast scan --rule missing-example crates` prints the remaining count, which only
  falls. The scan module and the diff reader carry examples on every public item already.
- INV-EXAMPLE-1 still holds. An example on a supported direct-use surface, such as `scan` or
  `hunks`, calls it directly and is recorded in the `[examples]` table of
  `contract/PUBLIC_SURFACE.toml`. Any other item's example goes through `script!`.
- An edit inside an older item does not require its example. The rule judges the line the item's
  name is on, so a new item or a changed signature does.

## Correspondence

Refines ADR 0011: the docs bar now covers rustdoc as well as Markdown pages. Relates to
ADR 0001 and ADR 0002, since the rule reads Rust through the one parser.

## Verification

- The `examples` lane and its CI step "New public items show an example".
- `sim_scan_examples`: `sim_every_drawn_file_agrees_with_the_model` and
  `sim_the_same_seed_replays_to_one_trace` over 2,048 seeds, and
  `the_shipped_scan_holds_a_change_to_its_added_lines` against the binary.
- `hunks::tests` and `scan::example::tests::only_library_modules_are_judged`.
