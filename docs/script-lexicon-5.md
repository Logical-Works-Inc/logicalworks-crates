# The `script!` lexicon, page 5

<!-- Generated from `LEXICON` in crates/lgwks-ast/src/script/lexicon.rs by the test `the_lexicon_page_is_rendered_from_the_lexicon` in lgwks_macros. Edit the table, not this page. -->

Pages: [page 1](script-lexicon.md) · [page 2](script-lexicon-2.md) · [page 3](script-lexicon-3.md) · [page 4](script-lexicon-4.md) · page 5.

## `await approval`

Opens a block: its line ends in `:`. Calls `approval_at`.

- `await approval <label> within 30s:` — the block's bytes parked for a human decision, bound as `Approved`; the payload returns once decided

Refuses: an `await approval` with no `within`; a label that is not one word.

Guarantees: a first run parks without holding a task, and a resume after the decision returns the payload (ephemeral); an `await approval` with no deadline is refused (idiomatic).

```text
flow ship(payload: Vec<u8>) -> Vec<u8>:
    let approved = await approval ship within 30s:
        payload
    give back approved.payload().to_vec()
```
