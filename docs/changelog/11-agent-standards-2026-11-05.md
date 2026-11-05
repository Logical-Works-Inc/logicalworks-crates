# lgwks_deps 6.1.0 / lgwks_std 4.0.2 / lgwks_ast 5.1.1 / lgwks_macros 2.0.2 / lgwks_bot 7.1.1 / drishti 0.1.2

Released 2026-11-05. The storefront now sells the open standards an agent harness speaks, and every crate names its new home, `Logical-Works-Inc/logicalworks-crates`.

## lgwks_deps 6.1.0 (minor)

- Eight new default-off features, recorded in ADR 0015 and #869. `mcp` brings the MCP SDK (rmcp 3.9.0) with server, client, stdio, child-process and streamable HTTP transports and its OAuth client. `model` brings one client for every model provider (genai 0.7.3).
- `json-schema` validates JSON Schema 2020-12 (jsonschema 0.60.2). `oauth2` runs the authorization-code flow with PKCE (oauth2 5.0.0). `cose` builds COSE_Sign1 envelopes (coset 0.4.2) around `lgwks_std::sign`.
- `merkle-log` keeps the RFC 6962 append-only log (ct-merkle 0.3.0, hashed with lgwks_std's BLAKE3). `otel` exports OpenTelemetry over OTLP/HTTP and bridges `lgwks_std::trace` spans (opentelemetry 0.33.1, tracing-opentelemetry 0.34.0).
- `tls-ring` is the one in-process TLS provider those clients share: rustls on ring, with no C toolchain. `storefront::tls::install_ring` installs it once per process, and `lgwks tls [--json]` makes that call from the shell. `agent` selects every feature above.
- Each engine is reached through `lgwks_deps::storefront`, never as a direct edge. Thirteen approvals carry their measured retire costs and lifecycle rows, and the catalog has eight new rows.
- Four seeded simulations replay hostile inputs through these journeys. Merkle roots are checked against an RFC 6962 reference hash, and tool calls against a model of their schema. COSE receipts are tampered with and opened by a second tenant, and 1,000 threads race the first TLS install.
- `lgwks new` pins `lgwks_bot` 7.1.1 and `lgwks_std` 4.0.2.

## lgwks_std 4.0.2, lgwks_ast 5.1.1, lgwks_macros 2.0.2, lgwks_bot 7.1.1, drishti 0.1.2 (patch)

- `repository` and `homepage` name `Logical-Works-Inc/logicalworks-crates`, and so do the crate READMEs, the register review links and most doc links. Pages still owed a size or prose split keep the old link, which GitHub redirects. The published site moves to `logical-works-inc.github.io/logicalworks-crates`.

## How a `script!` flow uses them

A `script!` body is plain Rust expressions, so a flow calls a storefront path directly and no new word is needed. The `script_json_schema` journey validates a tool call inside a flow this way, and ADR 0015 records why a word waits until `lgwks_bot` owns the capability.
