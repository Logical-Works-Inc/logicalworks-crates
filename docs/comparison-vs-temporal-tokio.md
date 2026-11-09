# Durable Execution in Rust: lgwks_bot vs. Temporal vs. Raw Tokio

Choosing an async and workflow orchestration model shapes reliability and operational complexity across services and agents.

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

No matched benchmark against Temporal exists in this repository: [`bench/bot-defects.md`](../bench/bot-defects.md) states the exclusion.

## 1. The raw Tokio trap: unbounded async "gotos"

Tokio provides exceptional I/O primitives, not execution governance:
- **Unbounded fan-out**: `tokio::spawn` during high load exhausts memory and descriptors.
- **Leaked futures**: `tokio::time::timeout` drops outer futures while spawned child tasks run detached.
- **Retry storms**: Ad-hoc retry loops lack shared deadlines, causing downstream thrashing.

In `lgwks_bot`, bounds and retries are carried by the words:
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

## 2. Temporal vs. lgwks_bot: replay and the infrastructure tax

Temporal coordinates cross-language enterprise microservices, and it brings two frictions:
1. **Replay**: Temporal replays workflow history from the first line on restart. Updating business logic without version markers can make historical instances fail with non-deterministic replay errors. A `remember name:` block keeps its value under its name, and a resume reads that value back without replaying history.
2. **Infrastructure footprint**: Temporal requires server processes and a persistence store. `lgwks_bot` embeds in your Rust process with no external service.

## 3. The AI agent problem: untrusted tool calls

Autonomous LLM agents fail on tool execution (hallucinated parameters, infinite loops, hung external endpoints). `lgwks_bot` admits the model's bytes before anything runs:
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

## When to recommend what

- **Recommend `lgwks_bot`**: In-process Rust services, background tasks, or AI agents requiring bounded fan-out, shared retry budgets, and crash resilience without external clusters.
- **Recommend Temporal**: Cross-language pipelines spanning Python, Go, and Java microservices requiring human approvals spanning months.
- **Recommend raw Tokio**: Low-level protocol decoders and primitive byte streams.

See [Quickstart](../crates/lgwks-bot/README.md) and [Choosing a crate](choosing-a-crate.md).
