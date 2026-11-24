---
name: lgwks-script-flows
description: Use when writing or fixing a lgwks_bot::script! flow, or when the macro refuses a line. Gives the words (each, retry, within, together, step, graph, saga, admit, serve), Scope and Tenant, which FlowError retries, block_on, the emitted ARCHITECTURE, and how to read a compile error.
---

# Writing a flow

A flow is the one way to fan out, bound, retry and undo work. The runtime makes the small calls (spawn, retry, deadline, cancel), so write the word and not the mechanism.

```rust
use lgwks_bot::script::{FlowError, Scope, Tenant};

lgwks_bot::script! {
    /// Square every number, each attempt bounded, a transient failure retried.
    flow squares(numbers: Vec<u64>) -> u64:
        let squared = each number in numbers:
            retry up to 3 times, waiting 10ms:
                within 1s:
                    number.saturating_mul(number)
        give back squared.iter().sum()
}

fn main() -> Result<(), FlowError> {
    let scope = Scope::root(Tenant::new("acme")?);
    let total = lgwks_bot::script::block_on(squares(&scope, vec![1, 2, 3]))?;
    assert_eq!(total, 14);
    Ok(())
}
```
Modelled on the compiled quickstart in `crates/lgwks-bot/README-script.md`. Every flow takes a `Scope` first and returns `Result<Output, FlowError>`. The block also emits a constant `ARCHITECTURE`, built from the same tokens, which is the flow's map.

## The words

Each line starts with a word or is a line of Rust. A line ending in `:` opens a block, indented four spaces. Forms and examples: `docs/script-lexicon.md` and its pages 2 to 5.

| You need | Write | Bound or rule |
|---|---|---|
| Every item at once | `each x in xs:` | The machine's width; results in input order; the first failure cancels the rest |
| A limit from outside | `each x in xs, at most (quota) at once:` | The bound is a named value in `1..=65536`, never a typed number |
| A deadline | `within 2s:` | `TimedOut` past it; a zero or a unitless duration is refused |
| Repeat on a transient failure | `retry up to 3 times, waiting 100ms:` | Attempts `1..=1000`; one key across attempts; inside the run's retry budget |
| Branches in parallel | `together:` | Each line concurrently; first failure cancels the rest |
| A named scope | `step name:` | Its own key and its own error location |
| One at a time | `for x in xs:` | In order, each in its own scope |
| A dependency graph | `graph:` with `node name needs a, b:` | A cycle, an unknown need or a duplicate is refused at the word |
| Undo on failure | `saga name:` with `compensate step:` | Undos run newest first, once each |
| A live feed | `whenever x in feed, buffering N, drop oldest:` | `N` in `1..=65536`; `drop newest` and `refuse` are the others |
| Untrusted output | `admit from model through (gate):` | A typed plan or a typed refusal |
| Another flow | `run other(args)` | Same tenant, same scope |
| Leave | `give back value`, `fail with reason`, `fail transiently with reason` | `give back` leaves the flow |

The durable, serving and process words have their own skills: `skills/lgwks-durable-runs/SKILL.md` (`remember`, `await approval`, `ask`, `send`) and `skills/lgwks-serve-an-app/SKILL.md` (`serve`, `route`).

## Errors decide the retry

`FlowError` is `#[non_exhaustive]` and every variant carries `at`, the step path. Retry is decided by the variant and never by the message (`crates/lgwks-bot/src/script/error.rs`).

| Variant | Retried by `retry` |
|---|---|
| `Transient`, `TimedOut` | Yes (`RetryClass::Safe`) |
| `Bot` | Only when its own class is `Safe` |
| `Failed`, `Cancelled`, `Refused`, `Blocked`, `Incompatible`, `Parked`, `Throttled`, `Exhausted` and the validation variants | No |

- Turn a foreign `Result` into the right class with `or_retry()` (transient) or `or_fail()` (permanent). `std::io::Error` already maps its timeout, reset and refused kinds to transient.
- `FlowError::transient(reason)` and `FlowError::failed(reason)` make one by hand.
- `Throttled` means the run's retry budget is spent: failures are systemic and another retry adds load. Do not wrap it in a second retry.
- Match on the variant. `is_retryable()` and `retry_class()` answer the question for you.

## When the macro refuses

A line that could end or leak the program is refused when the macro expands, and the refusal names the form to write. Each row is a refusal listed on the lexicon pages.

| The refusal | The repair |
|---|---|
| A concurrency bound typed as a number | Pass a named value: `at most (quota) at once` |
| An `await approval`, `ready` or `ask` with no deadline or request | Add the `within` or the `with (..)` the word names |
| A `serve` with no `at most (limit) runs in flight` | Add the ceiling |
| `give back` inside a block that has its own value | Move it to the flow's own level |
| A `node` outside a `graph`, a `compensate` outside a `saga`, a `route` outside a `serve` | Nest it under its opener |
| A step the saga never ran, a cycle in a graph | Name only earlier steps; remove the cycle |

Run `lgwks check app.rs` to see the same refusals with a line, a column and a suggested word, without building (`skills/lgwks-serve-an-app/SKILL.md`).

## Traps

- There is no `#[tokio::main]`. Entry is `lgwks_bot::script::block_on`.
- A flow's `Tenant` is a partition key, not an authenticated identity.
- Deadlines and stops are cooperative and take effect at an `.await`. A long synchronous stretch is not preempted.
- Do not spawn inside a flow. `each`, `together` and `graph` own their branches.
- A line of Rust that could end or leak the program is refused, and a panicking call is refused by its resolved path. Return a `FlowError`.

## Before you say done

- [ ] Every fan-out is an `each`, `together` or `graph`, and each bound is named.
- [ ] Every foreign error went through `or_retry()` or `or_fail()` on purpose.
- [ ] The flow compiles, and `lgwks check` on a one-file app prints OK.
- [ ] A test covers the failure path with its exact `FlowError` variant (`skills/lgwks-test-on-the-rails/SKILL.md`).
