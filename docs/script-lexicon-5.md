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

## `ask`

Stands inside a line of Rust. Calls `ask_at`.

- `ask <model> with (request)` — one model call under `request`: token budget and deadline, decoded into the declared type

Refuses: an `ask` with no `with (request)`.

Guarantees: a truncated or undecodable output is refused typed, never decoded into a claim (decoupled); an `ask` with no request is refused at the word, naming the form (idiomatic).

```text
flow draft(model: &lgwks_bot::proposal::StubModel) -> String:
    let reply: String = ask model with (lgwks_bot::script::AskRequest::default())
    give back reply
```

## `send`

Stands inside a line of Rust. Calls `send_at`.

- `send <receiver> with <payload>` — one outbound effect through `receiver`: intent first, dispatched once, settled from evidence

Refuses: a `send` with no `with <payload>`.

Guarantees: the intent is durable before dispatch, the unknown stays unknown, and nothing dispatches twice (ephemeral); a `send` with no payload is refused at the word, naming the form (idiomatic).

```text
flow mail(mailer: &StubMailer, payload: Vec<u8>) -> lgwks_bot::script::SendOutcome:
    let outcome = send mailer with (payload)
    give back outcome
```
