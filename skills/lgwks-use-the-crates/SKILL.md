---
name: lgwks-use-the-crates
description: Use when writing Rust that consumes these crates, here or in another repository, and you need the right front door. Routes a need to script!, lgwks_bot::kernel, lgwks_std, lgwks_ast or the lgwks_deps storefront, with the words script! accepts.
---

# Using the crates

Reach for the first row that fits, and never for the support code beneath it.

| You need | Use | Read |
|---|---|---|
| Fan-out, deadlines, retries, steps, sagas, an untrusted plan | `lgwks_bot::script!` | `docs/script-lexicon.md`, `crates/lgwks-bot/README-script.md` |
| A journal, broker, lease, gate, effect or clock under your own runtime | `lgwks_bot::kernel` | `docs/guides/lgwks-bot/kernel.md` |
| Hex, base64, ids, time, hashing, JSON, HTTP, logging, with no async runtime | `lgwks_std` | `docs/guides/lgwks-std/index.md`, `docs/catalog.md` |
| One AST across languages, or a scan of Rust source | `lgwks_ast` | `docs/guides/lgwks-ast/index.md` |
| A third-party stack such as `tokio`, `axum`, `rmcp` or `candle` | The `lgwks_deps` storefront | `docs/guides/lgwks-deps/storefront.md` |
| Proof that no unowned dependency entered a build | `lgwks-deps check .` | `docs/guides/lgwks-deps/index.md` |

Look a capability up rather than guessing its owner:

```sh
cargo run --locked -p lgwks_deps --bin catalog -- --json .
```
## Install

```sh
cargo add lgwks_bot
cargo add lgwks_std
cargo install lgwks_deps
```
The package is `lgwks_bot`, and `lgwks-bot` is not a package. The defaults carry almost everything: `lgwks_std` leaves only `http` out, and `lgwks_bot` leaves out `net`, `process`, `fs`, `edge`, `signal`, `inspect`, `ephemeral` and a few instruments. Feature tables are in `docs/guides/lgwks-bot/features.md` and `docs/guides/lgwks-std/reference.md`.

## A first flow

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
    let total = lgwks_bot::block_on(squares(&scope, vec![1, 2, 3]))?;
    assert_eq!(total, 14);
    Ok(())
}
```
Every flow takes a `Scope` first, carrying the tenant, the step path and the cancellation. The block also emits a constant `ARCHITECTURE`, which is its map.

## Rules that save a rewrite

- A concurrency number typed as a literal is a compile error. `each` picks the bound, and a limit from outside is written `at most (quota) at once`.
- Retrying is decided by the error's variant: `FlowError::transient` retries, and a permanent error never does. Do not match on a message.
- Untrusted model or tool output enters only through `admit`, which returns a typed plan or a typed refusal.
- A flow's `Tenant` is its host's partition key, not an authenticated identity.
- Capability gating is auditable authority, not a sandbox, because in-process code can always dial out.
- Deadlines and stops are cooperative, taking effect at an `.await`, so a long synchronous stretch is not preempted.
- There is no `#[tokio::main]` here. Entry is `lgwks_bot::block_on`.
- Do not add `tokio`, `serde` or `regex` as direct edges: `skills/lgwks-std-first/SKILL.md` maps each to its owner.
- A derive on a JSON type needs `#[serde(crate = "lgwks_std::json::serde")]`.

## What is promised

Only `script!`, `try_join!`, the `script` module and `lgwks_bot::kernel` carry a promise in `lgwks_bot`. Every other item can change in any release, so a consumer naming one pins an exact version. `docs/guides/index.md` lists the surfaces of the other crates.

A gap in these crates is an issue to file, not a private copy to keep.
