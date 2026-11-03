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
| `ubuntu-24.04-arm` (4 vCPU) | the Linux archives and shards (in `rust:1.99.1-trixie`, for util-linux 2.41 `setpriv` and Landlock, #337), the evidence job, gate, advisories, std-full, no-default, docs, feature matrix, target matrix, portable storefront legs, Pages, both check rows |
| `macos-15` (3 vCPU) | the macOS archives and shards (through `churn-slot`, INV-CI-SLOT-1), clippy, GPUI and Metal storefronts, AppCUI, the timing gates, Miri (the darwin seam) |
| `windows-latest` | GPUI Windows, process containment on Windows, AppCUI Windows |

### Compile once, test wide

Each suite (the workspace without lgwks_bot, and lgwks_bot full) is compiled
once per platform by `cargo nextest archive` (lane `test-archive`) and uploaded.
Its shards download the archive and run `--partition slice:N/M` on runners of
their own, so no shard compiles. The lane commands take the archive through
`${NEXTEST_BUILD:-<cargo flags>}`, so `ci-local` runs the same lanes from source.
The archive carries `target/debug/churn-slot` (`[profile.default.archive]` in
`.config/nextest.toml`), because an archive run resolves the wrapper inside the
extracted target. The acceptance-receipts binary is built by the evidence job
and handed to the required row as an artifact, off the run's critical path.

### Caching

One owner per layer, both in `hosted-rust`:

- **rust-cache** holds the registry and the dependency build, keyed by the
  lane group the job builds (`cache-key`), and saves from `main` alone.
  Before this, every ref saved: 9.71 GiB of 10 in 153 entries on 2026-11-03,
  and pull requests evicted main's entries and compiled cold.
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
| Generalized | Every lane runs on every pull request: every feature set, the target matrix, WASI, all 28 grammars, macOS, Linux and Windows. | parity: 66 lanes, 52 shared |
| Decoupled | A shard waits only on its archive; the check rows and the receipt read what other jobs wrote and build nothing. | `needs:` in `ci.yml` |
| Ephemeral | A hosted runner is destroyed after its job. The caches are caches: losing one costs a cold build and never changes an outcome. | `hosted-rust` |
| Portable | arm64 Linux natively and in trixie, arm64 macOS, x86_64 Windows, and `cargo check` across the declared target matrix. The wall check reads GNU and BSD `date`. | `check-target-matrix`, the `ci-wall` lane |
| Multi-tenant | Runs of different pull requests share no machine and no writer; a pull request's runs supersede rather than race, and only main writes a cache. | `concurrency:` in `ci.yml` |
| Fastest | ≤ 5 min wall, cold and warm, on passing runs. | the required row's `ci-wall` step |

### Runs

The run table is [`ci-runs.md`](ci-runs.md), kept whole so this file stays
inside the size law.
