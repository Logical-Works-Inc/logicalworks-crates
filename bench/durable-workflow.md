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
cargo run --locked --release --manifest-path bench/Cargo.toml --bin lgwks-bench-durable -- --runs 1000
cargo run --locked --release --manifest-path bench/Cargo.toml --bin lgwks-bench-durable -- --runs 200 --recover --recover-rounds 5 --json
```
`--json` prints one document on stdout. Progress goes to stderr. Exit 2 is a bad command line. Exit 1 is a failed run. The same `--seed` gives the same failure plan.

The cargo bench target runs the same binary. It passes every argument after `--` on, drops the `--bench` flag cargo adds, and exits with the binary's exit code.
```sh
cargo bench --locked --manifest-path bench/Cargo.toml --bench durable -- --rows lgwks,restate --recover
```
## The Restate row
`--rows lgwks,restate` runs the same workload on `restate-server` and prints one table. `--markdown` prints the table as Markdown. `--restate-server <path>` names the server binary (default `restate-server` on `PATH`).
```sh
cargo build --locked --release --manifest-path bench/Cargo.toml
cargo run --locked --release --manifest-path bench/Cargo.toml --bin lgwks-bench-durable -- --rows lgwks,restate --recover --markdown
```
Build all bins first. The driver starts `lgwks-bench-restate-worker` from the folder that holds the driver binary, and `cargo run` of one bin does not build the other. `cargo bench` builds every bin.
- The driver starts the server with its own config. It reads back `restate-server --dump-config` and refuses to run if the WAL or its fsync is off. The row prints the setting it found.
- The worker is the same ten steps on `restate-sdk` 0.13.0. The driver draws the failure plan from the seed with the same code as the lgwks row and gives it to the worker as a file.
- The retry policy is Restate's own: 1 ms first delay, factor 2, 5 attempts. The lgwks row retries in the process.
- Restate has no tenants. Runs are keyed `<tenant>-<run>`, so one tenant never reads another's run.
- Only the server is killed in the recovery rounds. The worker stays up. The time includes the server restart, the log replay and the 5 ms approval of every parked run.
- The worker logs each step body once per execution. The driver checks the log: a step that ran twice after recovery is a failure.
- Peak RSS of the Restate row is the server plus the worker. The lgwks row is one process.
## Results
One run, macOS aarch64 (Apple silicon), `lgwks_bot` 9.0.0, `restate-server` 1.8.1, `restate-sdk` 0.13.0, release build, seed 1, 1000 runs, 5 recovery rounds.

The table between the two marker lines is written by the binary, never by hand. This one command runs both rows and replaces it:
```sh
cargo run --locked --release --manifest-path bench/Cargo.toml --bin lgwks-bench-durable -- --rows lgwks,restate --recover --write-doc bench/durable-workflow.md
```
`--write-doc` needs `--rows`. It checks the doc before the run and refuses one that lacks a marker line, repeats one or has the end first. A marker line inside a code fence does not count. The write goes to a temp file, then a rename.

<!-- durable-table:start -->
| measure | lgwks | restate |
|---|---|---|
| engine | lgwks_bot 9.0.0 | restate-server 1.8.1 + restate-sdk 0.13.0 |
| tenancy | two tenants: a host and a run store each | none: one workflow per run, keyed <tenant>-<run> |
| durability | run store on disk, one file per tenant; sync_all on every group-committed batch; not configurable | wal on, fsync on (log-server, provider replicated) |
| runs asked, finished (samples) | 1000, 939 | 1000, 1000 |
| throughput | 455.97 runs/s | 219.02 runs/s |
| latency p50 / p95 / p99 (max) | 128.0 / 159.1 / 184.2 ms (240.4) | 220.9 / 995.6 / 1157.8 ms (1196.6) |
| peak RSS (ps, after the run) | 9.2 MB (this process, ps after the run) | 316.6 MB (restate-server plus the worker, ps after the run) |
| retries taken, failures injected | 408, 469 | 489, 489 |
| runs given up: retry budget, every attempt failed | 61, 0 | 0, 0 |
| application code lines of the flow | 70 | 105 |
| kill -9 to every run done, p50 over the rounds | 258.5 ms (5 rounds, 4762 runs parked in all) | 2396.1 ms (5 rounds, 4998 runs parked in all) |
| step bodies run twice after recovery (checked) | 0 (42858) | 0 (44982) |
| deploy: start to ready (samples) | 11.5 ms p50 (1 sample) | 588.3 ms p50 (6 samples) |
<!-- durable-table:end -->

The deploy row is the time from start to ready, read from the same `Clock` as every wait in the run. Each cell is the p50 and its sample count.
- lgwks: opening both tenants' hosts and run stores, until the rig can take its first run. One sample per run of the binary.
- Restate: from spawning `restate-server` until the worker is registered. With `--recover`, each server restart, from spawn until both doors answer, is one more sample. Five rounds make six samples.
- It is a start, not a build. Compile time is not measured.

Lgwks is ahead on this workload. Restate runs less than half the throughput, p50 is 1.7 times higher and p95 and p99 are over six times higher. Its server and worker use 34 times the memory, recovery takes nine times longer, and it takes 51 times longer to start. The lgwks row is one process and the Restate row is a server plus a worker over HTTP. That is the shape of each engine, and the table does not hide it.

About 60 of 1000 lgwks runs ended `Throttled`. The host gives each tenant a small retry budget, and a 20% failure rate spends it. These runs are counted and left out of the latency sample. They are never timed as finished. Restate has no such budget, so it finishes all 1000. The two rows do not run the same set of runs. The count of throttled runs changes a little from run to run.
## Limits
- Peak RSS comes from `ps` after the run. Stable Rust has no in-process peak reading.
- One run, one machine. The spread between runs is not measured.
- Restate runs in its default single-node setup. A multi-node cluster is not measured.
- The deploy row has few samples: one for lgwks, and one plus one per recovery round for Restate. A p50 of one sample is that sample.
- `restate-sdk` runs with `default-features = false` and the `aws_lc_rs` backend. Its default `rust_crypto` backend pulls `rsa` and `crypto-bigint`, which have published advisories. The bench never checks request identity, so the backend does not change what it measures.
