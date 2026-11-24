---
name: lgwks-serve-an-app
description: Use when exposing a lgwks_bot flow or per-tenant app over HTTP, scaffolding a one-file app, or reading what lgwks new, check, expand or run print. Covers serve and route, the App builder, tenancy at the edge, budgets, authentication and the refusals before it serves.
---

# Serving an app

The edge is feature `edge`, off by default. A serving app needs `script` and `edge`, and the scaffold writes exactly that.

## The one-file path

```sh
cargo install lgwks_deps
lgwks new demo
lgwks check demo
lgwks expand demo
lgwks run demo
```
`lgwks new <name>` writes two files and nothing else: a manifest that pins `lgwks_bot` and `lgwks_std` exactly, with `script` and `edge`, and `app.rs` holding one `script!` block that serves an echo route. It refuses a non-empty directory (`crates/lgwks-deps/src/bin/lgwks/new.rs`). The app reads `PORT` and falls back to 9000.

| Command | Does | Exit |
|---|---|---|
| `lgwks new <name>` | Scaffold the two files | 0, or 2 |
| `lgwks check [file]` | Parse and admit the file without building | 0 OK, 2 refused |
| `lgwks expand [file]` | Print the Rust the `script!` block compiles to | 0, or 2 |
| `lgwks run [file]` | Build and serve, printing build times | 0, or 2 |

Every command takes `--json`. A refusal has four fields, `location` (`line:col`), `kind`, `message` and `suggestion`. The `kind` is `unknown-word`, `bound-exceeded` or `malformed`, and a suggestion is the nearest word of the vocabulary. A file holds one `script!` block.

## The `serve` word

```text
flow drafts() -> u16:
    let served = serve on port from env PORT, at most 1000 runs in flight:
        route POST (body: DraftRequest) -> Draft:
            give back draft_reply(body).await?
    give back served.port()
```
The route line also names its path after the method (`docs/script-lexicon-5.md`). The `at most N runs in flight` ceiling is required and lies in `1..=65536`.

- Two tenants on one route never read each other's state.
- Past the ceiling the edge answers 503 with Retry-After and queues nothing.
- A retry with the same idempotency key returns the first result and never runs the body twice.
- A body that does not decode is answered 400 naming the field, before the handler runs.
- A served flow arms `SIGTERM` and `SIGINT` before it binds, and either drains the serve (ADR 0078).

## The `App` builder

Use it when a tenant's state is a plain value and a handler is a pure transition: `Fn(&mut S, Input) -> Vec<Effect>`, synchronous, with no await, no lock and no I/O.

```text
use lgwks_bot::app::{App, Effect, Method, Principal, RequestHeaders};

let app = App::new("whoami")
    .state::<u64>()
    .authenticate(|tenant: &str, headers: &RequestHeaders<'_>| {
        let key = headers.bearer()?;
        if key == format!("key-{tenant}") {
            Principal::new(tenant)
        } else {
            None
        }
    })
    .route_as(Method::GET, "me", |_: &mut u64, who: &Principal, (): ()| {
        vec![Effect::reply(200, who.as_str().as_bytes().to_vec())]
    });
assert!(app.check().is_ok());
```
This is the doctest on `App::route_as` in `crates/lgwks-bot/src/app.rs`, which compiles only with `edge` on, so it is shown here as text. The path is written without its leading slash, and `normalise_path` adds it. Then `app.serve(listener, &stop)` checks again and serves until the `EdgeStop`.

| Builder call | Effect |
|---|---|
| `route(method, path, handler)` | Body decoded as JSON `T`; `GET` and `POST` only |
| `route_as` and `authenticate` | A refusal is 401 with nothing read or queued; a head past 64 headers or 16 KiB is 431 |
| `on(tag, handler)` | Takes the answer to an effect the handler asked for |
| `grant(cap)` and `provider(p)` | An effect whose cap is not granted is answered `Refused` and never reaches the provider |
| `budget(BudgetLimits::new(units, window, tenants), clock)` | A unit allowance per tenant per window; one past it is `Refused` |
| `approvals(store)` | Parks `AwaitApproval` through the same store a flow's approval uses |
| `command_limits(limits)` | Bounds `Effect::Command` on its own pool |

A request names its tenant in its path, as `t/{tenant}/{rest}` under a leading slash, and the health route names none and is answered by the edge itself. Each tenant owns its own state.

## What `check` refuses before it serves

- A name outside 1..=64 bytes, more than 256 routes or handlers, or a path past 256 bytes.
- Two routes on one verb and path.
- A handler that returns an effect whose cap was not granted.
- A `route_as` with no authenticator.
- A budget with a zero window or zero tenants.

A plan from a model goes through `App::from_plan` and the same check. A plan holds at most 64 KiB.

## Traps

- A handler that blocks stalls its tenant's worker. Put slow work behind `Effect::Command` and a `Provider`.
- Handlers run once per message and a client may retry, so a command honours the idempotency key in its body (ADR 0025).
- `authenticate` compares secrets however its author writes it, so compare a digest.
- One transition dispatches at most 8 effects and the rest are dropped and logged.

## Before you say done

- [ ] `lgwks check` prints OK, and `app.check()` is `Ok`.
- [ ] A test sends the same key twice and the body ran once.
- [ ] A test with two tenants on one route proves neither sees the other.
