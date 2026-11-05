# Durable Execution in Rust: lgwks_bot vs. Temporal vs. Raw Tokio

Choosing an async and workflow orchestration model shapes reliability and operational complexity across services and agents.

| Property | Raw Tokio | Temporal (Rust SDK) | `lgwks_bot` |
|---|---|---|---|
| **Primary Abstraction** | Bare async futures & tasks | Activity & Workflow stubs | Indented `script!` flows on ECS world |
| **Deployment Model** | In-process library | External server cluster + workers | In-process library |
| **Operational Overhead** | None | High (Cluster, DB, mTLS, metrics) | Zero (Runs embedded with `cargo add`) |
| **Scheduling Latency** | Sub-microsecond | 10ms - 50ms (gRPC network hops) | Sub-microsecond (Nanosecond in-memory ECS) |
| **Fan-Out Safety** | Manual (`Semaphore`) | Worker task queues | **Compiler-guaranteed** (`each x in xs:`) |
| **Cancellation** | Fragile (task leaks) | Server-coordinated | **Hierarchical & guaranteed** (`within D:`) |
| **Retry Budgets** | Manual loops | Exponential backoff | **Shared attempt budget** (`retry up to N`) |
| **Crash Resumption** | None (in-memory) | Replay event history | **Step-keyed journal** (`step name:`) |
| **Code Versioning** | N/A | **Fragile** (Replay non-determinism) | **Zero** (Honest state transitions) |
| **AI Agent Admission** | None | None | **Native** (`admit the model's plan:`) |

## 1. The Raw Tokio Trap: Unbounded Async "Gotos"

Tokio provides exceptional I/O primitives, not execution governance:
- **Unbounded Fan-out**: `tokio::spawn` during high load exhausts memory and descriptors.
- **Leaked Futures**: `tokio::time::timeout` drops outer futures while spawned child tasks run detached.
- **Retry Storms**: Ad-hoc retry loops lack shared deadlines, causing downstream thrashing.

In `lgwks_bot`, bounds and retries are enforced by syntax:
```rust
lgwks_bot::script! {
    flow process_orders(client: &OrderClient, orders: Vec<Order>) -> Vec<Receipt>:
        let receipts = each order in orders:
            retry up to 3 times, waiting 50ms:
                within 5s:
                    client.process(&order).await?
        give back receipts
}
```

## 2. Temporal vs. lgwks_bot: Replay Fragility & Infrastructure Tax

Temporal coordinates cross-language enterprise microservices, but introduces two major frictions:
1. **The Replay Trap**: Temporal replays workflow history from line 1 on restart. Updating business logic without strict version markers causes historical instances to fail with non-deterministic replay errors. `lgwks_bot` uses deterministic step keys (`step name:`) and event-driven ECS state transitions—persisting completed step outcomes without synthetic replay constraints.
2. **Infrastructure Footprint**: Temporal requires external Go servers, persistence stores (Cassandra/Postgres), and Elasticsearch. `lgwks_bot` embeds directly in your Rust process with zero external services.

## 3. The AI Agent Problem: Securing Untrusted Tool Calls

Autonomous LLM agents fail on tool execution (hallucinated parameters, infinite loops, hung external endpoints). `lgwks_bot` provides formal admission barriers:
```rust
lgwks_bot::script! {
    flow execute_agent_plan(sandbox: &Sandbox, plan: AgentPlan) -> ExecutionSummary:
        step verify_plan:
            admit plan.is_policy_compliant()?
        step run_tools:
            let results = each tool in plan.tools:
                retry up to 2 times, waiting 100ms:
                    within 10s:
                        sandbox.dispatch_tool(&tool).await?
        give back ExecutionSummary::new(results)
}
```

## When to Recommend What

- **Recommend `lgwks_bot`**: In-process Rust services, background tasks, or AI agents requiring bounded fan-out, shared retry budgets, and crash resilience without external clusters.
- **Recommend Temporal**: Cross-language pipelines spanning Python, Go, and Java microservices requiring human approvals spanning months.
- **Recommend Raw Tokio**: Low-level protocol decoders and primitive byte streams.

See [Quickstart](../crates/lgwks-bot/README.md) and [Choosing a crate](choosing-a-crate.md).
