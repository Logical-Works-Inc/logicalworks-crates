# SKILLS: your first session in this repository

An agent that opens this repository reads this page, then only the one skill its task names.

`AGENTS.md`, `CODEBOOK.md`, `WORKFLOW.md` and `GOVERNANCE.md` hold the long rules, and this page holds no second copy of them.
A test reads this page and every skill against the tree, so a path, lane, binary or package named here exists (INV-DOC-7).

## The first five minutes

1. Read `INVARIANTS.md` whole before the first edit. Each line names a rule, why it exists and its enforcing test.
2. Read the `INVARIANTS.md` of the crate you touch, such as `crates/lgwks-bot/INVARIANTS.md`.
3. Look before writing: `contract/CATALOG.toml` lists every capability and its owner, and `docs/catalog.md` is its render.
4. Load the skill your task names below, and follow its checklist in order.
5. Run the gate lane nearest your change before you claim anything.

## Route your task

| Your task | Load |
|---|---|
| Add or change a public item, module, binary, feature or test | `skills/lgwks-change-a-crate/SKILL.md` |
| Write code that uses these crates, here or in another repository | `skills/lgwks-use-the-crates/SKILL.md` |
| The task suggests `tokio`, `regex`, `uuid`, `serde_json` or another familiar crate | `skills/lgwks-std-first/SKILL.md` |
| Add, upgrade or audit a third-party dependency | `skills/lgwks-dependency-admission/SKILL.md` |
| A gate lane or a CI row is red | `skills/lgwks-red-gate/SKILL.md` |
| Cut, publish or verify a release | `skills/lgwks-release/SKILL.md` |

## The map

| Package | Directory | Binaries | What it owns |
|---|---|---|---|
| `lgwks_std` | `crates/lgwks-std` | none | Primitives with no async runtime required |
| `lgwks_ast` | `crates/lgwks-ast` | `lgwks-ast` | The one parser, the `script!` reader, the zero-gate scan |
| `lgwks_deps` | `crates/lgwks-deps` | `lgwks-deps`, `lgwks`, the gate binaries | The storefront, the dependency gate, `ci-local` |
| `lgwks_macros` | `crates/lgwks-macros` | none | The syntax of `script!`, used through `lgwks_bot` |
| `lgwks_bot` | `crates/lgwks-bot` | `record-journal`, `recorded-instant` and others | `script!`, the ECS world, the driver, `lgwks_bot::kernel` |
| `drishti` | `crates/drishti` | `drishti` | The frontend's tokens, ink effects and residency model |

A package is `lgwks_std` with an underscore, its directory is `crates/lgwks-std` with a hyphen, and a binary is `lgwks-deps` with a hyphen.
`cargo add lgwks-std` finds nothing.

| Path | Holds |
|---|---|
| `contract/` | `APPROVED.toml` for edges, `LIFECYCLE.toml` for exits, `CATALOG.toml` for capabilities, `PUBLIC_SURFACE.toml` for `pub` ceilings |
| `scripts/gate-lanes.toml` | The one definition of the gate |
| `docs/README.md` | The index of every document, and the source `llms.txt` is generated from |
| `docs/adr/README.md` | The decision records and the fields each must hold |
| `docs/guides/index.md` | Consumer guides and the supported interface |
| `docs/releasing.md` | The release process |

## Commands that work

```sh
./scripts/ci-local.sh --list                        # every lane id
./scripts/ci-local.sh --fast                        # the lanes marked fast
./scripts/ci-local.sh --lane doc-governance         # one lane, as CI runs it
cargo nextest run --locked -p lgwks_deps -E 'test(/^agent_start_page::/)'  # one module
cargo run --locked -p lgwks_deps -- check .         # the dependency gate
cargo run --locked -p lgwks_deps --bin llms-txt     # regenerate llms.txt
cargo run --locked -p lgwks_deps --bin catalog -- --write .   # re-render docs/catalog.md
cargo run --locked -p lgwks_ast --features tool,scan --bin lgwks-ast -- scan crates
```
Use `cargo nextest`, because `cargo test` alone is only for the `doc-tests` lane. Pass `--locked` to every cargo call.

## Traps that cost a session

- **The supported surface is small.** `script!`, `lgwks_bot::kernel`, the parser and `lgwks_ast::scan`, and the gate and `storefront` of `lgwks_deps`. Every other item is `#[doc(hidden)]` support code with no promise (INV-SURFACE-1).
- **Every `pub` needs its ceiling.** A library file with no row in `contract/PUBLIC_SURFACE.toml` is held to zero `pub` items, and a ceiling above the count is refused too.
- **Every new `pub` shows an example.** The `examples` lane judges added lines, and `lgwks_ast` and `lgwks_macros` are held whole (INV-DOC-6).
- **A doc page has a shape.** It opens with `# ` and then a sentence, each fence names a language, and added prose holds no sentence past 45 words (INV-DOC-5).
- **A new Rust block in a doc can be refused.** One that names a workspace crate without `script!` is capped per file (INV-EXAMPLE-1), so use `sh`, or write the block with `script!`.
- **Every Markdown file holds at most 120 lines.** Split a long page, keep one entry in `docs/README.md`, and link the parts from it, or `llms-txt` refuses them.
- **A guide's `path:line` citation is pinned.** After a file moves, `check-doc-citations --update` records it and prints what changed for review.
- **Defaults are on.** A capability is off by default only where ADR 0003 records its cost or the target denying it (INV-DEFAULT-1).
- **A decision needs a record.** It takes a numbered page in `docs/adr/` and a row in `docs/adr/README.md`, whose index a suite test reads.
- **The repository is public.** Name no private host, path or consumer in a commit, page or test.
- **A docs-only pull request is narrow.** Only `docs/**/*.md`, root pages other than `README.md`, and `llms.txt` skip the suites (INV-CI-DOCS-ONLY-1). A `skills/` page runs them all.
- **A green local run covers only its own lanes.** Lanes marked `ci` run on CI alone, and the local receipt names them (INV-GOV-1).
- **`--no-default-features` removes more than it sounds.** On `lgwks_bot` it drops `script!`, the ECS substrate and the driver. It is the process-only build (ADR 0007).

## Before you say done

1. The lanes your change touches ran, and you read their output.
2. Every capability you added is reached from a real run path, and a test drives it through its public interface.
3. Every doc, `CHANGELOG.md` row and `INVARIANTS.md` line you affected still tells the truth.
4. The pull request is merged, or the order was only to open it. State which, with the commit.
