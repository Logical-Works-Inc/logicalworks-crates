---
name: lgwks-durable-runs
description: Use when a lgwks_bot flow must survive a crash or restart, run once per request key, wait for a human approval, or keep records sealed at rest. Covers Host, Task, the run store, remember, resume, replay drift, idempotent submit, approvals and the sealed feature.
---

# Durable runs

A host runs a task and returns a report. With a run store installed, each `remember` step is recorded, and a resume replays the record and never polls the step's future again. The second submission of one key below reattaches and runs nothing.

```rust
use lgwks_bot::kernel::task::{Host, RequestKey, Submission, task};
use lgwks_bot::script::Scope;

lgwks_bot::script! {
    flow greet(name: &str) -> String:
        let line = remember hello:
            format!("hello, {name}")
        give back line
}

async fn run(dir: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    let host = Host::builder("acme")?.run_store(dir)?.build()?;
    let greeting = task("greet", |scope: Scope, name: &str| {
        let name = name.to_owned();
        async move { greet(&scope, &name).await }
    })?;
    let key = RequestKey::new("order-17")?;
    let first: Submission<String> = host.submit(&key, &greeting, "world").await?;
    assert!(matches!(first, Submission::Executed(_)));
    let again: Submission<String> = host.submit(&key, &greeting, "world").await?;
    assert!(matches!(again, Submission::Reattached(_)));
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("lgwks-skill-durable-{}", std::process::id()));
    let ran = lgwks_bot::script::block_on(run(&dir));
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    ran
}
```
Built on the module example in `crates/lgwks-bot/src/task.rs`, and compiled and run as a doctest. `submit` derives the run from the tenant and the key. A plain `host.run` on a store host must mint a fresh run id, which needs feature `ephemeral` (off by default). Without it the run is `Refused` with "could not mint a run identity", and no step runs. The promised path is `lgwks_bot::kernel::task`.

## The parts

| Part | What it does |
|---|---|
| `Host::builder(tenant)` | One tenant per host; `build()` refuses a zero deadline or a ceiling past `MAX_ADMITTED_TASKS` |
| `.run_store(dir)` | A file store, one file per tenant, opened and replayed at install, so a bad directory fails here |
| `.records(arc)` | Your own `RunRecords` store; `Host::run_store()` then answers `None` |
| `.clock(clock)` | The clock every run's deadline reads; a virtual one makes a deadline free to exhaust |
| `.durable_codec(id)` | The schema id of your durable values, `lgwks.bot.schema.v1.<kind>` by convention |
| `host.run(&task, input)` | A `Report<O>`: `disposition()`, `output()`, `error()`, `run_id()`, `trail()` |
| `host.resume(run, &task, input)` | Replays the recorded steps, runs the rest |
| `Scope::business_key(name, input)` | A digest stable across runs, for work that must happen once per business fact |

A report never claims an external effect happened: a local task reports `EffectKnowledge::None`.

## Resume and drift

A resume under a changed definition is refused before any step is polled, as `FlowError::Incompatible`. The `Drift` says which axis moved, and each has its own repair.

| `Drift` | Meaning | Repair |
|---|---|---|
| `Definition` | The declared revision differs | Start a new run |
| `Input` | The input digest differs | Decide whether the same work applies |
| `Order` | The count of durable steps differs | Start a new run, or restore the old shape |
| `Schema` | The value codec id differs | Migrate, or restore the codec |

An old store file is refused at open with `StoreError::FormatVersion`. There is no migration: keep your copy and re-run. Two adjacent steps swapped without a change of count are not detected, and the source says so (`crates/lgwks-bot/src/task/definition.rs`).

A second resume of a run already resuming on the host is `FlowError::RunInFlight`, and nothing runs. A host at its resume ceiling answers `ResumesSaturated`.

## One run per request

`host.submit(&RequestKey::new("order-17")?, &task, input).await` derives the run from the tenant and the key and records the input digest before any step runs.

| Result | Meaning |
|---|---|
| `Submission::Executed` | First submission; the body ran or resumed |
| `Submission::Reattached` | Same key and input already finished; the record comes back and the body never runs |
| `Submission::InFlight` | An earlier client left before an outcome; an effect may be live |
| `RequestError::Conflict` | Same key, different input; both digests ride with it |
| `RequestError::NoStore` | The host has no store; a durable submission needs one |

A stop after admission and a refusal before it are not recorded as the verdict, so a restart cannot poison the key.

## Approval

`await approval ship within 30s:` parks the block's bytes and holds no task. The first run ends in `FlowError::Parked`. A person decides through the host, then the flow resumes.

- `host.pending(limit)` lists `PendingApproval`s; `host.decide(id, ApprovalDecision::approve(by, digest))` or `ApprovalDecision::deny(by, digest)` answers one. The digest must match the parked payload.
- A resume after the decision returns the payload, or `ApprovalDenied`, or `ApprovalExpired` if the deadline passed on the host clock. None is retryable.
- `max_pending_approvals` caps undecided approvals per tenant.
- An action that hands an effect out of the process with no matching approval is `NeedsApproval`.

## Authority and repair

A task declares its needs, a host grants a `GrantSet`, and a missing capability is `FlowError::Blocked` carrying the whole `Deficit`. With `repair_ledger(dir)` the report carries a `RepairTicket`, and `repair_bounds(attempts, spend)` bounds a run's life so repeated refusals end.

## Sealed records

Feature `sealed` (off by default) seals journal and run-store records at rest under fresh data keys, through `lgwks_bot::kernel::sealed`. A sealed store answers absence like a plain one, and a key fault fails the pending listing (`docs/adr/0065-a-sealed-store-answers-absence-like-a-plain-one.md`).

## Traps

- A name is a label, not an identity or a grant.
- The body's future need not be `Send`; borrow the input only before the future is built.
- Do not retry a `Parked`, `Incompatible` or `Conflict`. They are answers.
- A run that waits on an admission permit is charged to its deadline.

## Before you say done

- [ ] A resume test replays a recorded step and polls its future zero times.
- [ ] Two tenants on one directory never read each other's run.
- [ ] A changed revision, input and codec each fail as their own `Drift`.
