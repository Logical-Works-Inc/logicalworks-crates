# Workflow §12: the CI specification

Moved verbatim from `WORKFLOW.md` on 2026-10-22, when every Markdown file in a
repository was held to 120 lines (Director, 2026-10-10). `WORKFLOW.md` is the
index and keeps the § number. Rewritten on 2026-11-03 for GitHub-hosted runners
(province#9); the self-hosted layout it replaced is in `ci-runs.md` and git.

---

## 12. CI: the specification

`.github/workflows/ci.yml` and `.github/actions/hosted-rust/action.yml`
implement this section. A change to either is held to it, and a number below is
replaced only by a newer measurement of the same thing, with its run id.

### The budget

| Measure | Bound | Source |
|---|---|---|
| Wall clock of a run, warm or cold | ≤ 5 min | Director 2026-10-26 ("CI FAILING IF OVER 5 MINUTES") and 2026-11-03 (every gate under 5 min, cold and warm); held by the `ci-wall` step of the required row |
| Wall clock of one job | ≤ 5 min | `timeout-minutes: 5` on every job |
| Outcome | every lane passes | Director: "U CANNOT CLAIM SPEED OF CODE IF UR FUCKING CI DOESNT PASS" |

*Wall clock* is the attempt's `run_started_at` to the required row's last step,
read from the Actions API, queueing included. *Cold* means no rust-cache entry
and no sccache entry for the job's key; *warm* means main saved both. A bound is
met by a run that passes; a red run's wall clock is not evidence of anything.

### Where jobs run

Every job runs on a GitHub-hosted runner, and none names `self-hosted`
(INV-CI-RUNNER-CLASS-1, rule 5 of `check-gate-parity`). The repository is
public: hosted runners cost nothing, and a self-hosted runner would execute a
fork's pull request on the Director's Mac.

| Runner | Jobs |
|---|---|
| `ubuntu-24.04-arm` (4 vCPU) | the Linux shards (in `rust:1.99.1-trixie`, for util-linux 2.41 `setpriv` and Landlock, #337), the workspace build and simulation evidence, the evidence job, gate, advisories, std-full, no-default, docs, feature matrix, target matrix, portable storefront legs, the timing gates, Pages, both check rows |
| `macos-15` (3 vCPU) | the macOS shards (through `churn-slot`, INV-CI-SLOT-1), clippy, GPUI and Metal storefronts, AppCUI, Miri (the darwin seam); `ld64.lld` is the toolchain's `rust-lld` |
| `windows-latest` | GPUI Windows, process containment on Windows, AppCUI Windows |

### A docs-only change

The `changes` job lists a pull request's files (ADR 0011). When every one is a
`docs/**/*.md` page, a root page other than `README.md`, or `llms.txt`, the
suites are skipped and the gate decides. Its `check-docs` step holds every page
the change touches to the docs bar. `Build and workspace tests` then reads the
classification and the gate, and still holds the 300 s wall. A push to `main`
runs everything.

### Each shard builds what it runs

Each suite (the workspace without lgwks_bot, and lgwks_bot's `bot-rest` and
`bot-it-*` groups) is compiled in the job of each shard that runs it: lane
`suite-build` lists the binaries, which builds them and runs none, and the run lane
then takes `--partition slice:N/M`. This replaced a build-once archive whose upload,
second runner and download put 60 to 90 s between the compile and the tests (run
41671270220). The lane commands take the selection through
`${NEXTEST_BUILD:-<cargo flags>}`, so `ci-local` runs the same lanes from source. The
first Linux shard of each suite still archives what it built (`simulation-archive`),
and `simulation-evidence` lists those archives instead of compiling the tests again.
An archive carries `target/debug/churn-slot` (`[profile.default.archive]` in
`.config/nextest.toml`), because a run from one resolves the wrapper inside the
extracted target. The acceptance-receipts binary is built by the evidence job
and handed to the required row as an artifact, off the run's critical path.
The `compile` lane (every target, benches and examples included) runs nothing,
so it builds under `ci-bot`, dependencies at opt-level 0: under the test
profile it was cancelled cold at the limit on main (run 41793810191), and main
never saved the cache pull requests restore.

### Caching

One owner per layer, both in `hosted-rust`:

- **rust-cache** holds the registry and the dependency build, keyed by the
  lane group the job builds (`cache-key`), and saves from `main` alone.
  Before this, every ref saved: 9.71 GiB of 10 in 153 entries on 2026-11-03,
  and pull requests evicted main's entries and compiled cold. The key's hash
  is the action's own, over what the dependency build is compiled from (ADR
  0028), because rust-cache's hash moved on any manifest or config edit, and a
  miss deleted every restored artifact older than a week. A pull request that
  moves a registry package misses that key, so a read-only restore then lays
  main's newest entry for the group under it, and cargo rebuilds only what
  moved (ADR 0036). The `it` group rows pass their group cfg through
  `scripts/it_group_rustc.rs` to workspace crates alone and share one `bot-it`
  entry: as RUSTFLAGS it put 11 copies of one build past the 10 GB cap (ADR 0037).

The scaffolded-app journeys (lane `app-builds`) run on two `tests_linux` rows of
their own, `apps run` and `apps clippy`. Each nests a cargo build of a new app's
whole tree, and in a workspace shard the two held one lock for 100 s.
- **sccache** (v0.18.0, GitHub Actions backend) holds compiled crates. Every
  hosted job checks out at the same path, so workspace crates hit as well as
  registry crates. `CARGO_INCREMENTAL=0`, because sccache cannot cache an
  incremental compile and a hosted target dies with its job. Each archive job
  prints `sccache --show-stats` (the hit rate), and the required row prints
  the repository's cache usage.

Tools arrive prebuilt and pinned through `taiki-e/install-action`
(`nextest@0.9.149`, `cargo-audit@0.22.2`), never `cargo install`.

### The nine axes

| Axis | Requirement on the CI | Evidence |
|---|---|---|
| Frontier | Measured against the 13-instance self-hosted layout it replaces: main 41569522596 passed at 280 s warm, and cold pushes failed at 289 to 364 s. | the PR's before/after table, `ci-runs.md` |
| Hyperscale | Every resource is bounded: `timeout-minutes` on every job, nextest `slow-timeout` with `terminate-after` on every test, artifacts with a retention, caches saved from one ref. Shards add runners, not load on one machine. | `ci.yml`, `.config/nextest.toml` |
| Idiomatic | One definition of the gate: `scripts/gate-lanes.toml`. CI steps carry the lane commands verbatim and `check-gate-parity` refuses drift. Standard actions only. | `cargo run --locked -p lgwks_deps --bin check-gate-parity` |
| Generalized | Every lane runs on every pull request that changes code: every feature set, the target matrix, WASI, all 28 grammars, macOS, Linux and Windows. A docs-only change runs the gate lanes (ADR 0011). | the counts `check-gate-parity` prints |
| Decoupled | A shard builds and runs alone; the check rows and the receipt read what other jobs wrote and build nothing. | `needs:` in `ci.yml` |
| Ephemeral | A hosted runner is destroyed after its job. The caches are caches: losing one costs a cold build and never changes an outcome. | `hosted-rust` |
| Portable | arm64 Linux natively and in trixie, arm64 macOS, x86_64 Windows, and `cargo check` across the declared target matrix. The wall check reads GNU and BSD `date`. | `check-target-matrix`, the `ci-wall` lane |
| Multi-tenant | Runs of different pull requests share no machine and no writer; a pull request's runs supersede rather than race, and only main writes a cache. | `concurrency:` in `ci.yml` |
| Fastest | ≤ 5 min wall, cold and warm, on passing runs. | the required row's `ci-wall` step |

### Runs

The run table is [`ci-runs.md`](ci-runs.md), kept whole so this file stays
inside the size law.
