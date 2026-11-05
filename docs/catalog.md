# Capability catalog

> Generated from [`contract/CATALOG.toml`](../contract/CATALOG.toml) by
> `lgwks-deps catalog --write`. Do not edit this page: change the register and
> re-render. The `catalog` lane refuses a page that differs from a fresh render.

Every capability this repository owns, the path that owns it, and whether it
is on by default. A row whose **Default** is `on` needs no feature flag: adding
the crate is the decision. An `off` row states why in ADR 0003.

| Capability | Owner | Crate | Feature | Default | Search words |
|---|---|---|---|---|---|
| build scripts | `lgwks_std::build` | `lgwks_std` | `build` | on | build script, build.rs, rerun-if-changed, OUT_DIR, cargo directive, input digest, build input |
| hash | `lgwks_std::hash` | `lgwks_std` | `hash` | on | hash, digest, blake3, sha2, sha256, fingerprint, content identity |
| trace | `lgwks_std::trace` | `lgwks_std` | `trace` | on | trace, tracing, logging, log, subscriber, debug output, println, eprintln |
| trace install | `lgwks_std::trace::install` | `lgwks_std` | `trace-install` | on | trace install, subscriber install, tracing subscriber, global default |
| hex | `lgwks_std::hex` | `lgwks_std` | `core` | on | hex, hexadecimal, encode hex, decode hex |
| encoding | `lgwks_std::encoding` | `lgwks_std` | `core` | on | base64, percent, percent-encoding, url encoding |
| json | `lgwks_std::json` | `lgwks_std` | `json` | on | json, serde, serde_json, deserialize, serialize |
| ron | `lgwks_std::ron` | `lgwks_std` | `ron` | on | ron, config notation, human-facing document |
| wire | `lgwks_std::wire` | `lgwks_std` | `wire` | on | wire, rkyv, binary serialization, zero-copy |
| http | `lgwks_std::http` | `lgwks_std` | `http` | off — ureq -> rustls -> ring compiles C per target, and scripts/check-target-matrix.sh builds lgwks_std with no flags as a required claim for every declared target: cargo check -p lgwks_std --target x86_64-unknown-linux-musl failed in 7 s for a missing x86_64-linux-musl-gcc. One flag: --features http, or --features full (ADR 0003). | http, https, get, post, request, ureq, reqwest, fetch |
| online | `lgwks_std::online` | `lgwks_std` | `online` | on | online, reachability, tcp probe, is it up, connect check |
| pattern | `lgwks_std::pattern` | `lgwks_std` | `pattern` | on | regex, pattern, match, replace, capture |
| glob | `lgwks_std::glob` | `lgwks_std` | `core` | on | glob, wildcard, star match, *.rs |
| time | `lgwks_std::time` | `lgwks_std` | `core` | on | time, rfc 3339, timestamp, date, calendar, instant, chrono |
| id | `lgwks_std::id` | `lgwks_std` | `random` | on | id, uuid, identifier, unique id |
| random | `lgwks_std::random` | `lgwks_std` | `random` | on | random, entropy, getrandom, urandom, csprng, seed |
| seeded | `lgwks_std::seeded` | `lgwks_std` | `core` | on | seeded, deterministic, simulation seed, xoshiro, splitmix |
| sign | `lgwks_std::sign` | `lgwks_std` | `sign` | on | sign, signature, ed25519, seal, verify sealed record |
| similarity | `lgwks_std::similarity` | `lgwks_std` | `core` | on | similarity, cosine, jaccard, score, distance |
| fs | `lgwks_std::fs` | `lgwks_std` | `core` | on | fs, filesystem, walk, walkdir, directory walk, read dir, openat, handle-relative |
| task | `lgwks_std::task` | `lgwks_std` | `core` | on | task, block_on, executor, spawn_blocking, join_all, pool |
| process | `lgwks_std::process` | `lgwks_std` | `process` | on | process, kill, process group, subprocess, pgid, job object, reap |
| retry | `lgwks_std::retry` | `lgwks_std` | `core` | on | retry, backoff, exponential, attempts, deadline |
| leb128 | `lgwks_std::leb128` | `lgwks_std` | `core` | on | leb128, varint, variable length integer |
| script | `lgwks_bot::script` | `lgwks_bot` | `script` | on | script, script!, flow, orchestration language |
| async | `lgwks_bot::rt` | `lgwks_bot` | `rt` | on | async, runtime, tokio, spawn, supervisor, cancel |
| inspect | `lgwks_bot::inspect` | `lgwks_bot` | `inspect` | off — native tree-sitter grammars cannot build for wasm32-wasip1, which the bot's default set builds; `--features inspect` (or `full`) turns it on and the suite runs it there (ADR 0003). | inspect, code check, structural check, safe review |
| authority | `lgwks_bot::cap` | `lgwks_bot` | `always` | on | authority, grant, capability, permission |
| journal | `lgwks_bot::journal` | `lgwks_bot` | `always` | on | journal, durable, effect record, replay, crash recovery |
| ecs | `lgwks_bot::ecs` | `lgwks_bot` | `ecs` | on | ecs, schedule, change detection, tick, observe evaluate execute |
| sealed records at rest | `lgwks_bot::sealed` | `lgwks_bot` | `sealed` | off — Journal and run-store records sealed under fresh ChaCha20-Poly1305 data keys with key destruction as erasure; never built unless selected. One flag: --features sealed (ADR 0003). | sealed, sealed records, encryption at rest, shred, crypto-erase, ttl erasure, gdpr erasure |
| parse | `lgwks_ast::parser` | `lgwks_ast` | `parser` | on | parse, parser, ast, tree-sitter, ast-grep, syntax |
| scan | `lgwks_ast::scan` | `lgwks_ast` | `scan` | on | scan, zero gate, source detectors, lint |
| script reader | `lgwks_ast::script` | `lgwks_ast` | `script` | on | script reader, script parse, lexicon |
| dependency admission | `lgwks_deps` | `lgwks_deps` | `always` | on | dependency, admission, approved, edge owner, lgwks-deps check |
| storefront | `lgwks_deps::storefront` | `lgwks_deps` | `always` | on | storefront, install and select, feature selection |
| async storefront | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `tokio-full` | off — the crate's no-flag build is a required claim for every declared target; tokio-net, tokio-process and tokio-signal have no wasm32-wasip1 surface. One flag: --features tokio-full (ADR 0003). | tokio, async engine, runtime storefront, tokio-net, tokio-process |
| ecs storefront | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `bevy-app` | off — the storefront's default is the empty selection; the Bevy runtime set is one flag away and lgwks_bot reaches the ECS it needs through bevy-ecs (ADR 0003). | bevy, ecs storefront, bevy-app, bevy-time, bevy-state |
| native tui | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `appcui` | off — a native terminal UI cannot build for wasm32-wasip1, which the crate's no-flag build claims; measured 35 s warm. One flag: --features appcui (ADR 0003). | tui, terminal ui, appcui, widgets |
| gpu ui | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `gpui` | off — objc2 refuses to compile off Apple targets, so a default carrying it would break Linux and Windows; measured 122 s warm on macOS. One flag: --features gpui (ADR 0003). | gpui, gpu ui, desktop ui, zed |
| ml inference | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `ml-candle` | off — an inference runtime plus a network-capable tokenizer closure, measured 90 s warm and paid by every job that builds the crate; ml-candle-metal builds only on macOS. One flag: --features ml-candle,ml-tokenizers (ADR 0003). | ml, inference, candle, tokenizers, huggingface |
| zoned time (interim) | `lgwks_deps::storefront::jiff` | `lgwks_deps` | `time-jiff` | off — DST-aware calendar arithmetic and IANA zones need a database lgwks_std::time does not carry; interim opt-in until ELIMINATE lands. One flag: --features time-jiff (ADR 0003). | zoned time, timezone, dst arithmetic, jiff, calendar arithmetic, rfc 9557 |
| durable sqlite | `lgwks_deps::sqlite` | `lgwks_deps` | `sqlite` | off — One pinned SQLite with the estate's opened-connection defaults (WAL, synchronous FULL, busy timeout, user_version migrations); the bundled C build is native-only. One flag: --features sqlite (ADR 0003). | sqlite, rusqlite, durable store, durable sqlite, wal, user_version, migration |
| durable sqlite engine | `lgwks_deps::storefront::rusqlite` | `lgwks_deps` | `sqlite` | off — The engine the durable store opens: reached only through the storefront, never declared directly. Same flag as the defaults above: --features sqlite (ADR 0003). | rusqlite, sqlite engine, storefront rusqlite |
| http server | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `axum` | off — revenue-slice HTTP routing; axum never builds unless selected. One flag: --features axum (ADR 0003). | axum, http server, routing, web server, tower |
| http middleware | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `tower-http` | off — cors, trace and compression layers for the revenue slice; never built unless selected. One flag: --features tower-http (ADR 0003). | tower-http, tower, middleware, cors, compression, trace layer |
| ai input guard | `crates/lgwks-deps/src/lib.rs` | `lgwks_deps` | `llm-guard` | off — pure-Rust AI input scanners with no model download; never built unless selected. One flag: --features llm-guard (ADR 0003). | llm-guard, guardrails, prompt injection, ai security, input filter |
| documentation governance | `docs/adr/README.md` | `repository` | `always` | on | adr, architecture decision record, prd, product requirements, iso 42010, iso 29148, governance |
| sealed records | `lgwks_deps::storefront::chacha20poly1305` | `lgwks_deps` | `sealed-aead` | off — Authenticated encryption for sealed journal and run-store records has no std or estate equivalent; pure-Rust ChaCha20-Poly1305 with no C toolchain, never built unless selected. One flag: --features sealed-aead (ADR 0003). | aead, chacha20poly1305, chacha, poly1305, sealed records, authenticated encryption, encryption at rest |
| in-process tls | `lgwks_deps::storefront::tls` | `lgwks_deps` | `tls-ring` | off — The one TLS provider every agent-standards client shares: rustls on ring, installed once per process by tls::install_ring, with no C toolchain. Selected by the features that speak HTTPS. One flag: --features tls-ring (ADR 0015). | tls, https, rustls, ring, crypto provider, install_default, in-process tls |
| model context protocol | `lgwks_deps::storefront::rmcp` | `lgwks_deps` | `mcp` | off — The MCP SDK: server, client, stdio, child-process and streamable HTTP transports and the OAuth 2.1 client, on the ring provider; its upstream macros are declined. One flag: --features mcp (ADR 0015). | mcp, model context protocol, rmcp, tool server, tools/call, tools/list, json-rpc, agent tools |
| model client | `lgwks_deps::storefront::genai` | `lgwks_deps` | `model` | off — One client for every model provider with streaming and tool calls, on the ring provider. One flag: --features model (ADR 0015). | llm, model, genai, chat completion, anthropic, openai, deepseek, gemini, model provider |
| json schema validation | `lgwks_deps::storefront::jsonschema` | `lgwks_deps` | `json-schema` | off — JSON Schema 2020-12 validation for tool inputs and structured outputs, with remote $ref resolution off. One flag: --features json-schema (ADR 0015). | json schema, jsonschema, schema validation, 2020-12, tool input schema, structured output |
| oauth 2.1 with pkce | `lgwks_deps::storefront::oauth2` | `lgwks_deps` | `oauth2` | off — The authorization-code flow with PKCE that MCP authorization names, with no HTTP client of its own. One flag: --features oauth2 (ADR 0015). | oauth, oauth2, oauth 2.1, pkce, authorization code, csrf state, token exchange |
| cose receipts | `lgwks_deps::storefront::coset` | `lgwks_deps` | `cose` | off — COSE_Sign1 envelopes (RFC 9052) around lgwks_std::sign's ed25519 seal, so receipts have a standard wire form and one signer. One flag: --features cose (ADR 0015). | cose, cose_sign1, rfc 9052, cbor, signed receipt, envelope, coset |
| merkle log | `lgwks_deps::storefront::ct_merkle` | `lgwks_deps` | `merkle-log` | off — The RFC 6962 append-only log with inclusion and consistency proofs, generic over its digest (the journey hashes with lgwks_std's BLAKE3). One flag: --features merkle-log (ADR 0015). | merkle, merkle tree, rfc 6962, transparency log, inclusion proof, consistency proof, append-only log |
| opentelemetry | `lgwks_deps::storefront::opentelemetry` | `lgwks_deps` | `otel` | off — OpenTelemetry API and SDK with OTLP/HTTP export and the bridge from lgwks_std::trace spans, on the ring provider. One flag: --features otel (ADR 0015). | opentelemetry, otel, otlp, gen_ai spans, traces, metrics, tracing bridge, observability |

## When the gate refuses your code

The std-first gate refuses a capability hand-rolled beside the module that
owns it, and names the owner. That name is a row above: read the owner's
module documentation, or print the machine-readable catalog with
`lgwks-deps catalog --json`. The refusal and this page are held together by
the `catalog` lane, which refuses an owner the gate can name and this page
does not list.
