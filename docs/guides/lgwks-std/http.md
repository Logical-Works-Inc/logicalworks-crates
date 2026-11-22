# lgwks_std HTTP: pooling, streams, and private CAs

Blocking HTTPS for tooling and probes, rustls-only TLS, strict absolute-URL
validation. Three shapes: one shot, pooled, streamed.

## One shot

`get`, `post` and their `_with` forms build a fresh agent per call: no
pooling, no shared state. That is the cost model for a single shot.
Redirects follow to ten hops by default, headers strip across origins, and
HTTP statuses never error. Only transport failure, timeout and invalid URLs
do (`lgwks_std::http::Error` keeps its stage and class).

`Options` fields are private: set each through the builder method of the same
name. A `Response` reads through methods: `status()`, `truncation()`,
`headers()`, `header_values()`, `final_target()`, `redirect_chain()`, `body()`
and `text()`. `text()` returns `Err` for a body that is not UTF-8. `text_lossy`
is removed (5.0.0).

## Pooled bursts

Own a `Client` when a loop or a fan-out issues many exchanges:

```text
let client = Client::with_pool(Options::default(), PoolConfig::default())?;
let response = client.get_with(url, client.options())?;
```
One agent, many exchanges: a second request to a reached origin reuses the
idle keep-alive connection. `PoolConfig` sets the idle ceilings and the
idle age (ten connections, three per host, fifteen seconds: ureq's own
defaults). Clones share one pool. A per-call option that changes an
agent-level setting (timeouts, identity, pool sizes) runs that call on a
one-shot agent instead: still correct, just unpooled for that call. The
`http-pool` bench (`bench/http-pool/`) reports p50/p95/p99 for a 200-GET
loopback burst against the one-shot path.

## Streams, events, and pages

The call `Client::get_stream` leaves the final body on the wire under the same
ceiling (`max_body_bytes`, whole or preview). The adapter `into_sse` dispatches
its body as server-sent events. Comments are ignored, and `event`/`data`/`id`/`retry`
follow the specification. Multi-line data joins with `\n`, the last-seen id
persists, and a line longer than the ceiling is always refused. The walker
`paginate` follows a cursor-paginated listing to `None` under `PaginationLimits`. It has no offset
spelling, and its bound stops a server that never ends.

## Private CAs and client identity

`TlsOptions` replaces the default Mozilla roots with exactly the PEM it is
given, and pairs a chain with its key for mutual TLS. Every certificate is
structurally validated and the key is compared against the end-entity
certificate field by field before any socket opens; anything unprovable is
a typed TLS refusal, and debugging never renders key or certificate bytes.
Only PEM, only RSA/ECDSA (P-256/384/521): Ed25519 identity is refused with
its reason named.

## The async seam

There is no async streaming client here by decision. From async code, run
the blocking client below on a blocking thread and stream there; the
deadline still bounds the whole call, redirects included.

## Proof

Unit journeys stand up real loopback servers and count connections:
pool reuse, one-shot cost, expiry, clones, tenant isolation
(`http::tests::pool`); chunked and torn streams, ceiling outcomes, SSE
dispatch, pagination walks (`http::tests::streaming`); roots, key forms,
mismatches, Ed25519, truncation sweeps (`http::tests::tls`). Seeded
simulations sweep bursts and chunkings (`tests/it/sim_http_pool.rs`, `tests/it/sim_http_stream.rs`);
the mTLS handshake runs against a real `rustls` server with a private CA
that requires client certificates (`tests/it/sim_http_tls.rs`).
