# ADR 0015: the storefront sells the agent standards

Status: **accepted**, 2026-11-05. Implements #869.

Stakeholders: the Director; agent-harness authors (Larry, Rocco); `lgwks_bot` maintainers; storefront consumers.
Concerns: boilerplate every harness rewrites for each open protocol; duplicate engines beside estate owners; TLS providers that need a C toolchain.
Requirements: R2, R6, R9, R10, R13
Viewpoint: the dependency-admission viewpoint, mapping each open standard an agent speaks to one audited storefront edge.
Elements and relations: `lgwks_deps` features `tls-ring`, `mcp`, `model`, `json-schema`, `oauth2`, `cose`, `merkle-log`, `otel` and `agent`; `lgwks_deps::storefront::tls::install_ring` and the `lgwks tls` command that calls it; the estate owners `lgwks_std::sign`, `lgwks_std::json`, `lgwks_std::trace` and `lgwks_bot::rt`.

## Context

An agent harness speaks a fixed set of open standards. MCP carries its tools, a provider's chat API carries the model, and JSON Schema types the tool arguments. OAuth 2.1 grants delegated access, COSE signs receipts, an RFC 6962 log makes them tamper-evident, and OpenTelemetry carries the traces. None was in the storefront, so each harness would author its own edges and write its own glue.

Each candidate was checked against the estate first, with codegraph over `lgwks_std`, `lgwks_bot`, `lgwks_ast` and `lgwks_deps`. Ed25519 is `lgwks_std::sign`, BLAKE3 is `lgwks_std::hash`, and capability tokens are `lgwks_bot::cap_token`. The HTTP client is `lgwks_std::http` and the async runtime is `lgwks_bot::rt`. Those stay the owners and are not added again.

## Decision

1. **Seven standards, one feature each, default-off.** The features are `mcp` (rmcp 3.9.0), `model` (genai 0.7.3), `json-schema` (jsonschema 0.60.2), `oauth2` (oauth2 5.0.0) and `cose` (coset 0.4.2). Two more complete the set: `merkle-log` (ct-merkle 0.3.0) and `otel` (opentelemetry 0.33.1 with its SDK, the OTLP exporter and tracing-opentelemetry 0.34.0). `agent` selects all seven. Every edge is pinned exactly with its default features off. Each feature is off by default for the ADR 0003 reason, its measured cost, which `contract/APPROVED.toml` records per feature.
2. **One TLS provider.** `tls-ring` admits rustls on ring, the provider the sealed #728 exception already excuses, and builds reqwest with `rustls-no-provider`. The upstream default TLS of genai, jsonschema and rmcp is aws-lc-rs, which needs cmake and a C toolchain, so it is declined. The `reqwest` edge exists only to select that backend for the clients that bring reqwest. It is not re-exported, and `lgwks_std::http` stays the estate's HTTP client. `tls::install_ring` installs the provider once per process and refuses a different one installed first.
3. **Estate owners stay the owners.** COSE envelopes are signed and verified with `lgwks_std::sign`. JSON is built as `lgwks_std::json::Value` and converted at the boundary. Futures are driven on `lgwks_bot::rt`, and spans are emitted through `lgwks_std::trace`. The Merkle log hashes through `lgwks_std::hash` (BLAKE3) behind the `digest` traits, so no second hash crate enters.
4. **rmcp without its macros.** `#[tool]` and `#[tool_router]` expand to `::rmcp::` paths, which a consumer reaching rmcp through `lgwks_deps::rmcp` cannot resolve. `ServerHandler` and `ClientHandler` are implemented by hand through the storefront path, as the `mcp` journey does.
5. **No new `script!` words.** A `script!` body is Rust expressions, so a flow calls a storefront path directly. The `script_json_schema` journey validates a tool call inside a flow with no change to the language. A word belongs to the language only when `lgwks_bot` owns the capability it expands to, since each word is one `lgwks_bot::script` call. None of these seven engines is owned by `lgwks_bot`. When `lgwks_bot` takes one as an owned capability, such as an MCP tool source or a model provider, its word lands with it in the same change.

## Alternatives rejected

- **Each harness authors its own edges.** Every harness pays the boilerplate again, and the estate audits one copy of each engine per harness.
- **aws-lc-rs, the upstream default TLS.** It pulls aws-lc-sys, which needs cmake and a C toolchain on every target.
- **A second HTTP client re-exported beside `lgwks_std::http`.** Two clients would be two owners of one capability.
- **`opentelemetry-semantic-conventions`.** Its 0.33 release deprecated the GenAI keys, so the journey names the two keys it uses as constants.
- **a2a-rs for agent-to-agent calls.** It has one maintainer and about 12.8k downloads, which is too thin for an estate edge.
- **cedar-policy, cloudevents, ed25519-dalek and blake3 as new edges.** Each repeats an estate owner: `lgwks_bot::cap_token`, `lgwks_bot::journal`, `lgwks_std::sign` and `lgwks_std::hash`.

## Consequences

- A harness selects `agent` and writes no protocol boilerplate. Its tree resolves one reqwest, one rustls and one oauth2, with no aws-lc-sys, openssl-sys or cmake.
- The tree carries ecosystem version splits this repository cannot remove, such as digest 0.10 beside 0.11 and thiserror 1 beside 2.
- The ring exception's blast radius now names these features as well as `lgwks_std`'s `http`.
- Eleven of the thirteen edges have no workspace consumer yet, so their rows open with `policy-only:` until the harness built on them lands.

## Correspondence

Refines ADR 0003, under which a capability is on by default unless its cost is recorded: these features are default-off with recorded costs. Relates to ADR 0006 for the runtime the journeys drive on.

## Verification

- `crates/lgwks-deps/tests/storefront_consumers.rs` runs one offline journey per feature from `tests/fixtures/storefront-matrix`. The tests are `storefront_mcp_runs_its_journey`, `storefront_model_runs_its_journey`, `storefront_json_schema_runs_its_journey`, `storefront_oauth2_runs_its_journey`, `storefront_cose_runs_its_journey`, `storefront_merkle_log_runs_its_journey`, `storefront_otel_runs_its_journey` and `storefront_script_json_schema_runs_its_journey`.
- `the_agent_feature_resolves_no_c_toolchain_provider` refuses aws-lc-sys, openssl-sys and cmake in the `agent` tree.
- `the_storefront_matrix_default_graph_carries_no_optional_family` refuses every new engine in the default graph.
- `lgwks-deps check .` holds the thirteen approvals, their retire costs and their lifecycle rows.
