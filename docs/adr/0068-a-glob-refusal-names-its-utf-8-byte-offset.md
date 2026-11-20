# ADR 0068: a glob refusal names its UTF-8 byte offset

Status: **accepted**, 2026-11-19.

Stakeholders: the Director; a consumer that underlines the bad syntax in a pattern it shows a person; an agent that
slices the pattern at the reported offset.
Concerns: `PatternError::byte_offset` documents "the UTF-8 byte offset where the invalid syntax begins". A
descending range reported the scalar index of its `[` instead, so `é[z-a]` named byte 1 where the class begins at
byte 2. A consumer that sliced at that offset cut a scalar in half.
Requirements: INV-STD-SHARED-POLICY (`crates/lgwks-std/INVARIANTS.md`), #154.
Viewpoint: the glob compiler, as a consumer reads a refusal.
Elements and relations: `glob::compile_tokens`, `glob::compile_class`, `glob::byte_offset`, `PatternError`.

## Context

`compile_tokens` walks the pattern as scalars and converts each refusal offset with `byte_offset(chars, index)`.
The unclosed-class and double-star refusals did so. The class path passed the bare scalar `index` to
`compile_class`, which put it in the error unchanged. ASCII patterns hid the defect, because there a scalar index and
a byte offset are equal. rust-lang/glob reports a character position; Go's `path.Match` walks bytes. This contract
names bytes.

## Decision

1. **`compile_tokens` passes `byte_offset(chars, index)` to `compile_class`.** The caller holds both the scalar
   index and the scalars, so `compile_class` stays a compiler of one class body.
2. **`tests/it/sim_glob_refusal.rs` holds the strict compiler to a reference model** of the contract: for 4,096
   seeded patterns per seed, accept, kind and byte offset agree. Multi-byte prefixes, giant and empty patterns,
   and a replayed trace are their own families.

## Alternatives rejected

- **Recompute the offset inside `compile_class` from the whole pattern.** It needs the full scalar slice and repeats
  a sum the caller already owns. Reverse this if a refusal ever needs a position inside the class body: then
  `compile_class` owns the conversion.
- **Report a scalar index, as rust-lang/glob does.** The published field is named and documented as a byte offset,
  and a consumer slices `&str` by bytes.

## Consequences

- Every strict refusal now names a byte where `pattern.is_char_boundary(offset)` holds and the bad syntax begins
  (`sim_a_refusal_offset_is_a_boundary_at_the_bad_syntax`).
- ASCII patterns report the same offsets as before. Only patterns with multi-byte scalars before a descending class
  change, and they change to the documented value.
- The INV-STD-SHARED-POLICY line now names its enforcing test, `tests/it/tier_shared_policy.rs`, at its real path.

## Correspondence

Refines the glob contract of #154. Relates to ADR 0067, which landed in the same pull request.

## Verification

`crates/lgwks-std/tests/it/sim_glob_refusal.rs`: `sim_strict_refusals_match_the_reference_model`,
`sim_multibyte_prefixes_move_the_offset_by_their_bytes`, `sim_a_refusal_offset_is_a_boundary_at_the_bad_syntax`,
`sim_empty_and_giant_patterns_refuse_at_their_first_byte`, `sim_legacy_never_refuses_what_strict_refuses` and
`sim_the_refusal_trace_replays_and_seeds_diverge`.
