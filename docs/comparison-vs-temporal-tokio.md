# lgwks_bot, Temporal and raw tokio

This page compares three ways to run durable, bounded async work in Rust, and says when to pick each.

| Property | Raw Tokio | Temporal (Rust SDK) | `lgwks_bot` |
|---|---|---|---|
| **Primary abstraction** | Bare async futures and tasks | Activity and workflow stubs | Indented `script!` flows on one ECS world |
| **Deployment model** | In-process library | External server cluster plus workers | In-process library |
| **Operational overhead** | None | Cluster, database, mTLS, metrics | None beyond your process (`cargo add`) |
| **Scheduling cost** | The baseline | gRPC hops to the server; not measured here | Facade p50 1.02x–1.17x raw Tokio ([results](../bench/async/results.md)); an ECS tick 834 ns to 136 µs ([§4.9](production-readiness/4-9-performance.md)) |
| **Fan-out safety** | Manual (`Semaphore`) | Worker task queues | Enforced when the macro expands (`each x in xs:`); a typed number is refused |
| **Cancellation** | Manual; a spawned child can outlive its parent | Server-coordinated | Cooperative through the scope tree (`within D:`, on the scope's clock) |
| **Retry budgets** | Manual loops | Backoff policy per activity | One attempt budget (`retry up to N times:`) |
| **Crash resumption** | None (in-memory) | Replay of the event history | `remember name:` runs its block once per run; a resume reads the value back |
| **Changing the code** | N/A | Replay must stay deterministic across versions | A resume reads recorded values by name and replays no history; whether a changed flow still fits them is the author's review |
| **Untrusted model output** | None | None | `admit from model through (gate):` admits the bytes as a `Plan` or fails the step |

No matched benchmark against Temporal exists here. [`bench/bot-defects.md`](../bench/bot-defects.md) states the exclusion.

## 1. Raw tokio: I/O, not governance

Tokio gives you I/O primitives. It does not bound the work you build on them:

- **Unbounded fan-out**: `tokio::spawn` under load can exhaust memory and descriptors.
- **Leaked children**: `tokio::time::timeout` drops the outer future, and a child it spawned runs on.
- **Retry storms**: ad-hoc retry loops share no deadline, so a slow upstream gets more load.

In `lgwks_bot`, the words carry the bounds and the retries:

```rust
lgwks_bot::script! {
    flow process_orders(client: &OrderClient, orders: Vec<Order>) -> Vec<Receipt>:
        let receipts = each order in orders:
            retry up to 3 times, waiting 50ms:
                within 5s:
                    client.process(&order).await.or_retry()?
        give back receipts
}
```

## 2. Temporal: replay and a cluster

Temporal coordinates services across languages. It costs two things:

1. **Replay**: on restart, Temporal replays the workflow history from the first line. Changed logic with no version marker can fail that replay. A `remember name:` block keeps its value under its name, and a resume reads it back with no replay.
2. **Infrastructure**: Temporal needs server processes and a persistence store. `lgwks_bot` runs in your process with no external service.

## 3. Model output: admit before you run

A model can emit wrong parameters, loop, or call a hung endpoint. `lgwks_bot` admits the model's bytes before anything runs:

```rust
lgwks_bot::script! {
    flow execute_agent_plan(sandbox: &Sandbox, gate: &PlanGate, reply: Vec<u8>) -> Vec<ToolResult>:
        let plan = admit from model through (gate):
            reply
        let tools = sandbox.tools_of(&plan)
        let results = each tool in tools:
            retry up to 2 times, waiting 100ms:
                within 10s:
                    sandbox.dispatch_tool(&tool).await.or_retry()?
        give back results
}
```

## Which to pick

- **`lgwks_bot`**: in-process Rust services, background work or agents that need bounded fan-out, a shared retry budget and resume without a cluster.
- **Temporal**: pipelines across Python, Go and Java services, with human approvals that wait for months.
- **Raw tokio**: protocol decoders and byte streams.

See the [`lgwks_bot` quickstart](../crates/lgwks-bot/README.md) and [Choosing a crate](choosing-a-crate.md).
