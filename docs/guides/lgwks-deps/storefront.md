# lgwks_deps: storefront features

This page lists the features that bring an upstream crate in through
`lgwks_deps`, and what each one gives you.

## Storefront features

Each feature exposes its reviewed upstream capability through the storefront, so
you never declare it directly. `cargo add tokio` and `cargo add gpui` are refused
by the gate as second edges. A feature is on by default unless ADR 0003 records
its cost or a declared target denies it, and the default today selects none of them.

| Feature | Re-exports | Notes |
|---|---|---|
| `tokio` | `lgwks_deps::tokio` | The bypass path. Prefer `lgwks_bot::rt` unless you are deliberately working below the bot facade. |
| `tokio-time` | `lgwks_deps::tokio::time` | Enables Tokio timers. |
| `tokio-sync` | `lgwks_deps::tokio::sync` | Enables Tokio synchronization primitives. |
| `tokio-macros` | `lgwks_deps::tokio` | Enables Tokio's macros. Alias `lgwks_deps::tokio` as `tokio` at the consumer root before using `#[tokio::main]` or `#[tokio::test]`. |
| `tokio-io` | `lgwks_deps::tokio::io` | Enables asynchronous reader/writer traits and their extensions. |
| `tokio-net` | `lgwks_deps::tokio::net` | Enables Tokio sockets. |
| `tokio-process` | `lgwks_deps::tokio::process` | Enables Tokio child-process support. |
| `tokio-fs` | `lgwks_deps::tokio::fs` | Enables Tokio filesystem APIs. |
| `tokio-signal` | `lgwks_deps::tokio::signal` | Enables Tokio signal handling. |
| `tokio-full` | `lgwks_deps::tokio` | Combines the listed Tokio capabilities. |
| `bevy-ecs` | `lgwks_deps::bevy_ecs` | ECS substrate; no renderer, window, audio, or multithreaded scheduler. |
| `bevy-app` | `lgwks_deps::bevy_app` | Bevy `App` runtime, layered on `bevy-ecs`. |
| `bevy-time` | `lgwks_deps::bevy_time` | Bevy time resources and plugin, layered on `bevy-ecs`. |
| `bevy-state` | `lgwks_deps::bevy_state` | Bevy state runtime, layered on `bevy-ecs`. For `States` derives, alias `lgwks_deps::bevy_state` as `bevy_state` at the consumer crate root; the upstream derive resolves that manifest crate name. |
| `gpui` | `lgwks_deps::gpui` | Zed's GPU desktop UI framework, retaining its normal platform renderer. macOS builds require a usable Xcode Metal toolchain (`metal` and `metallib`); Linux builds require the platform development stack GPUI selects. External consumer checks run on Linux, macOS, and Windows. |
| `appcui` | `lgwks_deps::appcui` | Native terminal UI (appcui 0.5.10). Downstream code uses `use lgwks_deps::appcui; use appcui::prelude::*;` so upstream macros resolve their crate alias without a direct dependency. |
| `axum` | `lgwks_deps::axum` | Day-0 HTTP routing for the revenue slice: Tower-compatible routing with the http1, tokio, json and tracing stack. |
| `tower-http` | `lgwks_deps::tower_http` | Day-0 HTTP middleware: cors, trace and gzip/deflate compression layers. |
| `llm-guard` | `lgwks_deps::llm_guard` | Day-0 AI input guard: pure-Rust scanners with the fuzzy scanner only and no model download. |
| `ml-candle` | `candle_core`, `candle_nn`, `candle_transformers` | ML inference: tensor compute and transformer models. |
| `ml-candle-metal` | as `ml-candle`, plus Candle's macOS Metal backend | macOS-only. |
| `ml-tokenizers` | `lgwks_deps::tokenizers` | The matching tokenizer stack. |
| `time-jiff` | `lgwks_deps::jiff` | Zoned time and DST-aware calendar math (jiff 1). `lgwks_std::time` stays the UTC RFC 3339 core. |
| `sqlite` | `lgwks_deps::sqlite`, `rusqlite` | One SQLite engine (rusqlite 0.43) opened with estate defaults and `user_version` migrations. |
| `p256`, `libm` | `lgwks_deps::p256`, `libm` | ECDSA over P-256 for attestation evidence; portable float maths. Drishti uses both. |
| `desktop-preview` | `winit`, `softbuffer`, `cosmic-text` | The desktop preview window (cosmic-text 0.19). Native only. |
| `sealed-aead` | ChaCha20-Poly1305 | The engine `lgwks_bot`'s `sealed` feature seals records under. |
| `tls-ring` | rustls on ring | One TLS provider, installed by `lgwks_deps::storefront::tls::install_ring`, for the agent features below. |
| `mcp`, `model`, `json-schema`, `oauth2`, `cose`, `merkle-log`, `otel` | rmcp, genai, jsonschema 0.62, oauth2, coset, ct-merkle, OpenTelemetry | Agent standards (ADR 0015). `agent` selects all seven. |
| `macro`, `script`, `prettyplease`, `lgwks`, `debug-doctor` | `proc-macro2`, `quote`, `syn` | First-party tooling edges. `lgwks` and `debug-doctor` are the defaults. |

The facade-only consumer matrix compiles GPUI examples on Linux, macOS, and
Windows, and Candle Metal only on macOS.
It does not treat those platform-bound combinations as all-target features.

```toml
[dependencies]
lgwks_deps = { version = "8", default-features = false, features = ["tokio"] }
```
The ML features are off by default, so the default build compiles none of
Candle's closure. Selecting `ml-candle` pulls `hf-hub` through
`candle-transformers`. That makes the compiled tree network-capable, though
this repository's runtime loads a local checkpoint and never calls the hub.
`docs/candle-admission.md` records the authority and the transitive surface.
`gpui` is likewise outside the default build; `docs/bevy-admission.md` and
`docs/appcui-admission.md` cover the other two. Native terminal support does not
imply a full editor, backend execution, or measured platform parity.

If you want the tokio engine behind the bot facade, use `lgwks_bot::rt` rather
than `lgwks_deps::tokio`. The storefront path is for the case where you are
deliberately working below that facade.
