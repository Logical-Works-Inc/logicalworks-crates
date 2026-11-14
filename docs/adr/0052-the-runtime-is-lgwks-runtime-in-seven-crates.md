# ADR 0052: the runtime is lgwks_runtime in seven crates

Status: **accepted**, 2026-11-14, for #988 under #984.

Stakeholders: the Director; every consumer that names `lgwks_bot` today; the agent of each carve leaf (N2 to N12).
Concerns: the crate is a runtime and its name says "bot"; one crate of 141,902 source lines rebuilds whole on every
edit; the core boundary is a source-reading test, not a build edge.
Requirements: R1, R7, R11
Viewpoint: the package-dependency viewpoint: boxes are Cargo packages, arrows are `[dependencies]` edges, and an
arrow that is not drawn cannot compile.
Elements and relations: `lgwks_std`, `lgwks_kernel`, `lgwks_rt`, `lgwks_world`, `lgwks_session`, `lgwks_script`,
`lgwks_domains`, `lgwks_app`, `lgwks_runtime`, `lgwks_macros`; relations: `depends on`, `re-exports`.

## Context

Read on main `37fbb264`, 2026-11-14. These are the before numbers every carve prints against.

| Measure | Value | Command |
|---|---|---|
| Source | 141,902 lines, 343 files, 47 top-level modules | `git ls-files crates/lgwks-bot/src` with `wc -l` |
| Tests | 125,903 lines, 287 files | same, `tests` |
| Rebuild after a touch, dev, no compiler cache | 18.67 s wall, 51.59 s CPU, 1.23 GB peak RSS; the crate 5.5 s | `cargo build --locked -p lgwks_bot --features full --lib --timings` |
| Packages under `--features full` | 243 | `cargo tree --locked -p lgwks_bot --features full -e normal` |
| CI run 43581994689 | 88 jobs, 4 min 02 s wall, 9,475 runner s; 32 bot jobs take 4,314 | `gh run view 43581994689 --json jobs` |

The source and test lines were counted again for this record. `git diff --shortstat` from the empty tree to `HEAD`
printed `343 files changed, 141902 insertions` for `src` and `287 files changed, 125903 insertions` for `tests`.

ADR 0006 decision 4 says the core imports no driver. `tests/it/core_boundary.rs` holds it by reading source text.
Inside one crate any module can still name any other, so the rule is a test a later change can weaken.

## Decision

1. **The name.** `lgwks_bot` becomes `lgwks_runtime`. `rt` stays the name of the async driver inside it. `lgwks`
   alone stays free for a later umbrella.
2. **No shim.** One major, 9.0.0. No crate named `lgwks_bot` re-exports it, and no deprecation period runs.
3. **One front door.** `lgwks_runtime` is a facade. A consumer names only it. The crates below it exist for compile
   speed and hard boundaries, not as a second API.
4. **Seven crates.** The arrows below are the only edges Cargo allows. Each crate holds the modules on its line.

```text
lgwks_std
  └─ lgwks_kernel   clock, skew, cap, gate, cap_token, effect, journal, sealed, durable,
                    cas, tenant_store, dist_lease, broker, retry        (no async driver)
       ├─ lgwks_rt       runtime, time, sync, task, cancel, supervise, process, tenancy   (tokio lives here)
       ├─ lgwks_world    the owned ECS world, verb, spec, registry, bot tick, projection   (no driver)
       └─ lgwks_session  session, language, semantic, interface
  lgwks_rt + lgwks_world
       ├─ lgwks_script   script, task, proposal
       └─ lgwks_domains  domain/*, idempotent, inspect, review, frontier, stability, keyed
  lgwks_script + lgwks_domains
       └─ lgwks_app      app, cell, serve, edge, apps, estate, every bin   (axum lives here)
            └─ lgwks_runtime   facade: script!, kernel, world, app
```
5. **The modules the map does not name.** `json` goes to `lgwks_kernel`. `error` splits into one enum per crate
   (N3, #990). The `kernel` module becomes the facade path over `lgwks_kernel`. `ecs::requests` goes with the edge to
   `lgwks_app`. Each `sim_*` module moves with its subject. `lgwks_macros` stays its own proc-macro crate.
6. **The boundary is a Cargo edge.** `lgwks_kernel` and `lgwks_world` declare no tokio and no axum, so a driver
   import in the core fails to compile. That replaces the source reading of `core_boundary` once N5 and N8 land.

## Alternatives rejected

- **`lgwks_rt` as the name.** It collides with the driver module `rt`, so `lgwks_rt::rt` would read as a stutter.
- **`lgwks` as the name.** It is reserved for the umbrella over every estate crate.
- **Keep `lgwks_bot` for the verbs.** A stale `lgwks_bot` pin would then resolve and silently mean a smaller crate.
- **One crate with stricter tests.** The boundary stays a convention, and every edit still rebuilds 142k lines.
- **A crate per top-level module.** 47 crates means 47 release steps and 47 manifests to keep in agreement.

## Consequences

- Each carve follows the six carve rules of #984: `git mv`, one crate per pull request; the facade keeps every
  public path; tests move with their crate; a crate gets its own error enum, `INVARIANTS` rows and CI shard.
- Each carve prints cold workspace build, rebuild after a one-line edit in the moved crate, and that crate's suite
  wall, against the table above.
- The rename (N2, #989) needs a Director prompt that names `ci.yml`, `pages.yml` and `AGENTS.md`. The publish
  (N13, #1000) needs a Director publish order.
- Open: whether `lgwks_session` stays in this workspace, decided once #993 lists its callers.

## Correspondence

- Refines ADR 0006 decision 4: the decision stands, and its enforcement moves from a source-reading test to Cargo
  edges. Decisions 1 to 3 and 5 to 8 of ADR 0006 are untouched.
- Answers what ADR 0005 left open under its Consequences: the names of new crates. ADR 0005 stays superseded.
- `CODEBOOK.md` names the seven crates as planned until each carve lands.

## Verification

- The `doc-governance` lane (`check-docs`) holds this record's fields.
- `sim_the_adr_index_names_every_dropped_repeated_and_stale_row` holds its index row.
- Until N12 (#999) lands its edge-list test, decision 6 is `review only`, owner the agent of N12, dated 2026-11-14.
  `tests/it/core_boundary.rs` keeps holding ADR 0006 decision 4 until then.
