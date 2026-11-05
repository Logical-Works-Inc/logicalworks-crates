# Codebook: API shape, concurrency and observability

Part of [`CODEBOOK.md`](../../CODEBOOK.md), sections 4 to 6.

## 4. API shape

- Private by default. A `pub` field is a promise that any value of that type is
  valid; only make it when that promise is true.
- `#[must_use]` on anything whose return value is the point.
- Every public function with branching logic has unit tests covering each branch.
- New public API ships with a `/// ```rust` doc example so `cargo test --doc`
  compiles it, or with an example under `examples/`.
- Every `pub mod` listing carries a `///` on the declaration line.
- Semver is strict. Additive changes with defaults and new `#[non_exhaustive]`
  variants are minor. Removals and signature changes are major.
- Idempotent writes: an operation that may be retried is safe to retry. Document
  which operations are idempotent.
- Cursor-based pagination for lists. No offset pagination, no unbounded result
  sets.
- Deprecated APIs warn for two minor versions before removal, and the
  deprecation message carries the migration guide.

---

## 5. Concurrency and resources

- Structured concurrency: every spawned task has an owning scope, a cancellation
  token, and a bounded lifetime. `JoinHandle` is stored and awaited.
- RAII for all resources: files, connections, locks, handles, temp dirs are owned
  by a struct with deterministic cleanup in `Drop`. No `close()` a caller might
  forget.
- Document the global lock acquisition order. Prefer bounded `mpsc` over `Mutex`
  where a channel expresses the ownership. When a `Mutex` is necessary, hold it
  for the minimum critical section.
- Backpressure by design. Every queue, channel, and buffer declares capacity and
  overflow strategy at construction (bounded channel blocks, ring buffer
  overwrites oldest, error returned). Never discover the policy at 3am.
- Connection pools are bounded, with health checks, idle timeouts, and circuit
  breakers. No per-request connection establishment. No unbounded pool growth.

---

## 6. Observability

- `tracing` spans with correlation IDs, module owner, operation name, duration.
  No bare `println!` in library code. That is the PRINTS rule.
- Levels: `ERROR` broken contract, `WARN` degraded but serving, `INFO` lifecycle
  events, `DEBUG` developer diagnostics, `TRACE` wire-level.
- Service boundaries emit request latency histograms (p50/p95/p99), throughput
  counters, error rate by type, and queue depth or saturation gauge. Histograms,
  never averages.
- Health endpoints check real dependencies: liveness (process alive), readiness
  (dependencies reachable, can serve), startup (initialization complete). A 200
  that checks nothing is not a health endpoint.
