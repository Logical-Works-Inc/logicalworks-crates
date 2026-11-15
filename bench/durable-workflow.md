# Durable workflow
`lgwks-bench-durable` runs one flow of ten durable steps on the `lgwks_bot` engine. It measures what a user sees: runs per second, p50/p95/p99 latency, memory, retries, and the time to recover after `kill -9`.
## The flow
- Seven steps are local work. Each writes one record to an on-disk run store.
- Step 3 calls an outside service (2 ms). Step 6 calls a model (20 ms).
- 20% of attempts to these two calls fail. The crate's retry policy retries them (5 attempts, class `Safe`).
- Step 8 is an approval. The run parks. The bench grants it after 5 ms and resumes the run.
- Two tenants share the process. Each has its own host and its own store file.
- The two services are fixed-latency upstreams inside the process. The numbers show the engine, not a network.
- Every upstream wait and the approval delay are made on a `lgwks_bot` `Clock`. The binary passes the wall clock, so the latencies are real. The simulations pass a virtual clock and move it themselves, so no simulation waits real time.
## Reproduce
```sh
cargo build --locked --release --manifest-path bench/Cargo.toml --bin lgwks-bench-durable
bench/target/release/lgwks-bench-durable --runs 1000
bench/target/release/lgwks-bench-durable --runs 200 --recover --recover-rounds 5 --json
```
`--json` prints one document on stdout. Progress goes to stderr. Exit 2 is a bad command line. Exit 1 is a failed run. The same `--seed` gives the same failure plan.
## Results
One run, macOS aarch64 (Apple silicon), `lgwks_bot` 8.4.0, release build, seed 1.

| measure | value |
|---|---:|
| runs asked, finished (samples) | 1000, 939 |
| throughput | 453.24 runs/s |
| latency p50 / p95 / p99 | 128.2 / 163.3 / 186.9 ms |
| peak RSS (`ps`, after the run) | 8.2 MB |
| retries taken, failures injected | 410, 471 |
| runs the host's retry budget gave up | 61 |
| fsyncs, records staged | 648, 10 535 |
| kill -9 to every run done, p50 of 5 rounds (about 200 parked runs each) | 83.8 ms |
| step bodies run twice after recovery (8 892 checked) | 0 |

61 of 1000 runs ended `Throttled`. The host gives each tenant a small retry budget, and a 20% failure rate spends it. These runs are counted and left out of the latency sample. They are never timed as finished.
## Not done here
- No rival engine row exists yet. #1054 adds Restate on the same workload.
- Peak RSS comes from `ps` after the run. Stable Rust has no in-process peak reading.
- One run, one machine. The spread between runs is not measured.
