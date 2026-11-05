# The `script!` lexicon, page 4

<!-- Generated from `LEXICON` in crates/lgwks-ast/src/script/lexicon.rs by the test `the_lexicon_page_is_rendered_from_the_lexicon` in lgwks_macros. Edit the table, not this page. -->

Pages: [page 1](script-lexicon.md) · [page 2](script-lexicon-2.md) · [page 3](script-lexicon-3.md) · page 4 · [page 5](script-lexicon-5.md).

## `whenever`

Opens a block: its line ends in `:`. Calls `pump`.

- `whenever x in batches, buffering N, drop oldest:` (or `drop newest`, `refuse`) — each event of a live feed in arrival order, each in its own scope; `batches` is awaited for the next `Option<Vec<_>>` and `None` ends the feed; at most `N` events wait, and past that the policy decides; the value is the run's `PumpReport`

Refuses: a feed with no `buffering N`; a buffer outside `1..=65536`; an overflow policy other than `drop oldest`, `drop newest` or `refuse`.

Guarantees: never more than `N` events buffered, whatever the producer's rate; past it the declared policy, counted (hyperscale); events handled in arrival order, each in its own scope with its own key (generalized); a feed with no overflow policy is refused at the word, naming the three (idiomatic).
```text
flow watch(feed: &Feed) -> u64:
    let report = whenever tick in feed.next().await, buffering 1024, drop oldest:
        tick.record()
    give back report.handled
```

## `justify`

Opens a block: its line ends in `:`. Calls `Scope::with_justification`.

- `justify with (rules):` — the block in a scope that answers to `rules`: every step, node and retry attempt inside asks them first, and is slowed, parked or stopped as they decide

Refuses: a `justify` with no `with (rules)`.

Guarantees: a step the rules stop fails with `Justified`, naming the rule, before its body runs (decoupled); the rules govern only the block's own scope, never a sibling's (multi-tenant); a `justify` with no rules is refused at the word, naming the form (idiomatic).
```text
flow sync(api: &Api, rules: &Justify) -> u16:
    let code = justify with (rules):
        step call:
            api.call().await.or_fail()?
    give back code
```

## `remember`

Opens a block: its line ends in `:`. Calls `remembered_at`.

- `remember name:` — the block once per run; a resume reads its value back

Refuses: a name that is not one word.

Guarantees: a resume replays the value, never the block (ephemeral); a nameless `remember` is refused (idiomatic).
```text
flow ask(model: &Model) -> String:
    let reply = remember ask:
        model.ask().await.or_retry()?
    give back reply
```

## `admit`

Opens a block: its line ends in `:`. Calls `admit_at`.

- `admit from model through (gate):` (or `tool`, `document`) — the block's bytes admitted as a `Plan`; a refusal fails the step

Refuses: a producer other than `model`, `tool`, `document`.

Guarantees: a refusal is typed, at the step, with its provenance (decoupled); an `admit` with no gate is refused (idiomatic).
```text
flow plan(reply: &[u8], gate: &Gate) -> usize:
    let plan = admit from model through (gate):
        reply
    give back plan.wanted().len()
```

## `ready`

Opens a block: its line ends in `:`. Calls `Readiness::wait`.

- `ready x from (readiness) within 30s:` — the block once `readiness` is signalled, `x` bound to it

Refuses: a `ready` with no `within`.

Guarantees: released by the signal, never a poll; `TimedOut` past the deadline (hyperscale); a `ready` with no deadline is refused (idiomatic).
```text
flow rows(db: &Readiness<Pool>) -> u64:
    let n = ready pool from (db) within 30s:
        pool.awaited().count()
    give back n
```

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

## `fetch`

Stands inside a line of Rust. Calls `fetch_at`.

- `fetch <url> with (options)` — one HTTP exchange under `options`: deadline, attempt budget and body bound declared in the word
- `fetch <url>` — the same under defaults, inheriting the deadline and retries of the enclosing `within` and `retry`

Refuses: a `fetch` with no URL.

Guarantees: attempts share one budget per run and tenant, and an oversize body is refused while it streams (hyperscale); a `fetch` with no URL is refused at the word, naming the form (idiomatic).
```text
flow call(url: &str) -> u16:
    let answered = fetch url with (lgwks_bot::script::FetchOptions::default())
    give back answered.status()
```
