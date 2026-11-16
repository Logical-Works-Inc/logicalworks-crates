# ADR 0063: the durable bench rival is Restate, in the bench workspace

Status: **accepted**, 2026-11-15. Issue #1054, part of #789.

Stakeholders: anyone who reads the durable bench table; the estate dependency gate.
Concerns: a rival on another workload; fsync off on one side; competitor crates in the shipped graph.
Requirements: R9
Viewpoint: the measurement viewpoint, one command from the seed to the two-row table.
Elements and relations: `bench/Cargo.toml`, `bench/durable/rival*.rs`, `bench/restate/*.rs`, `bench/durable-workflow.md`.

## Context

#789 asks for the lgwks durable flow against an established durable-workflow engine, on one host and one workload.
The rival must run with no Python, no paid service and its log fsynced to local disk.

## Decision

1. The rival is Restate: `restate-server` 1.8.1 and a worker on `restate-sdk` 0.13.0 (pinned `=0.13.0`), plus `tokio`, which the SDK's server needs. The SDK runs with `default-features = false`, `http_server` and `aws_lc_rs`: its default `rust_crypto` backend pulls `rsa` (RUSTSEC-2023-0071) and `crypto-bigint` (RUSTSEC-2026-0387).
2. Both crates are edges of `bench/` alone. `bench/` is its own workspace root, so `lgwks-deps check` never sees them and `contract/APPROVED.toml` does not change.
3. The driver reads `restate-server --dump-config` and refuses to measure if the WAL or its fsync is off.
4. One command, `lgwks-bench-durable --rows lgwks,restate`, runs both rows from one seed and prints one table.

## Alternatives rejected

- **Temporal.** It needs a separate server, a database and a Go or Java worker for its stable path.
- **A Rust Durable Workflow client.** It needs its own server outside this host's stack, and no published rival row uses it.

## Consequences

- `bench/Cargo.lock` gains the SDK's graph, with `aws-lc-sys` (C) in place of `rsa` and `p256`. The estate crates' graph is unchanged.
- One release rebuild of the whole bench took 31.9 s wall, with a warm cache. Restate's p50/p95/p99, peak RSS and recovery time are printed beside the lgwks row, unfavourable rows kept.

## Correspondence

Follows the bench-workspace rule in `bench/Cargo.toml` and the dependency doctrine in `AGENTS.md`.

## Verification

- `sim_dump_config_decision_matches_the_reference_table` refuses every dump with the WAL or fsync off.
- `sim_table_text_markdown_and_json_agree_with_the_rows` holds the text, Markdown and JSON tables to the same numbers.
