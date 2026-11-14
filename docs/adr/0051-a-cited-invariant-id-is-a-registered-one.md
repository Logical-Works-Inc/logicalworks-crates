# ADR 0051: a cited invariant id is a registered one

Status: **accepted**, 2026-11-14, for #911.

Stakeholders: every reader sent from a code comment, a config reason or a doc to a rule; every
reviewer who trusts a citation.
Concerns: a citation that names a rule no register holds, so it claims an enforcement that does
not exist (SD-7).
Requirements: INV-RANDOM-1, INV-RT-BOUNDED
Viewpoint: the traceability viewpoint.
Elements and relations: the registers (`INVARIANTS.md` files, `docs/invariants/*.md`,
`contract/INVARIANTS.toml`) define ids; code, config and docs cite them.

## Context

#911 item 11 found `clippy.toml` citing INV-RT-BOUNDED and the std-first scan citing
INV-RANDOM-ONE-SOURCE. Neither had a register line. A sweep of the tree found 51 such citations.
Most were INV-RANDOM-ONE-SOURCE, a second name for INV-RANDOM-1.

## Decision

- A cited id must be defined: as `**INV-X**` in a register, as `id = "INV-X"` in
  `contract/INVARIANTS.toml`, or as `INV-X:` in a comment that states the rule beside its code.
- One rule has one id. INV-RANDOM-ONE-SOURCE is cited as INV-RANDOM-1 from now on.
- INV-RT-BOUNDED gets its register line in `crates/lgwks-bot/INVARIANTS.md`, enforced by
  `clippy.toml` through the `clippy-feature-bot` lane.
- History (changelog, governance ledger, ADRs) keeps ids as they were, and the guard does not
  read it.

## Alternatives rejected

- **Register each phantom as written.** Keeps two ids for one rule, and a reader still has to
  learn they are the same.
- **Review only.** That is how 51 phantoms came in.

## Consequences

- A new citation of an unknown id fails the lgwks_deps suite.
- `effect.rs` was past the size law, so no edit could fix its citation. Its id vocabulary
  moved to `effect/id.rs`. `pub use` keeps every path.

## Correspondence

`effect::id` is added to the CODEBOOK.md module table.

## Verification

- `tests/it/invariant_citations.rs`: `every_cited_invariant_id_is_defined_by_a_register` reads
  the tree, and `the_id_reader_tells_a_definition_from_a_citation` pins each form.
