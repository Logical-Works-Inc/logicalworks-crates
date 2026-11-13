# Architecture decision records

Every architecture decision in this repository is a record here, and the record
is held to **ISO/IEC/IEEE 42010** by `check-docs` in the `doc-governance` lane.
Prose review does not catch a missing field; a refusal does.

| Field | 42010 clause | What it must say |
|---|---|---|
| `Status:` | 5.2 — an AD identifies its state | `proposed`, `accepted`, `superseded`, `rejected` or `deprecated`, and the `YYYY-MM-DD` it was decided |
| `Stakeholders:` | 5.1 — stakeholders | who holds a concern about this decision |
| `Concerns:` | 5.1 — concerns | the interest in the system this record answers |
| `Requirements:` | 6.5 — rationale | the `REQUIREMENTS.md` ids (`R1`) the decision serves; a decision that traces to nothing is not one |
| `Viewpoint:` | 5.2 — viewpoints | the model kind and conventions the view is written in |
| `Elements and relations:` | 5.2 — views | the elements (crates, modules, features) and the relations between them the record describes |
| `## Context` | 6.5 — rationale | the situation the decision answers |
| `## Decision` | 6.5 — rationale | what was decided, in the imperative |
| `## Alternatives rejected` | 8.2 — rationale | each credible alternative, why it lost, and what it cost |
| `## Consequences` | 8.2 — rationale | what the decision costs, including what stays open |
| `## Correspondence` | 7.3 — correspondence | the other records this one supersedes, refines or relates to |
| `## Verification` | 8.3 — conformance | the test, gate or lane that holds the decision true, or `review only` with its owner |

## Rules

1. Numbered, append-only: `NNNN-short-name.md`, four digits, one decision each.
2. **A record is superseded, never rewritten.** A later decision adds a record
   that names the one it supersedes in its `## Correspondence`. The superseded
   record's `Status:` changes to `superseded` and nothing else in it changes.
3. A decision that changes the architecture without a record is a change nobody
   can review after the fact, and the `doc-governance` lane refuses the tree for
   the missing *field*, not for the missing judgement: review is still the owner
   of whether the decision was right.
4. The measurements a record cites are commands and their output. An estimate
   is labelled one and is replaced by a measurement before the claim stands.

## Index

| Record | Decision |
|---|---|
| [0001](0001-one-rust-reader.md) | One Rust reader, `lgwks_ast` |
| [0002](0002-scan-routed-through-lgwks-ast.md) | The scan reads Rust through `lgwks_ast` |
| [0003](0003-every-capability-on-by-default.md) | Every capability is on by default |
| [0004](0004-documentation-governance.md) | ADRs are 42010, PRDs are 29148, both machine-checked |
| [0005](0005-bot-core-is-an-event-driven-ecs.md) | The bot core is an event-driven ECS; tokio and the executors are drivers below it (superseded by 0006) |
| [0006](0006-one-ecs-world-one-tokio-axum-driver.md) | One ECS world is the core; one driver on tokio and axum carries events in and effects out |
| [0007](0007-process-only-defaults.md) | Process-only defaults hold without the substrate |
| [0008](0008-script-is-the-entrypoint.md) | Every capability is an SDK, an entrypoint and a `script!` word, or named here with why not |
| [0009](0009-human-approval-parks-the-run.md) | A human decision parks the run; `act` is gated by policy |
| [0010](0010-estate-uuid-replaces-the-uuid-crate.md) | The estate UUID issues v7 infallibly and packs as an FDB tuple element; serde is declined |
| [0011](0011-docs-only-changes-run-the-docs-gate.md) | A docs-only change runs the docs gate, not the suites; `check-docs` reads every page as its reader does |
| [0012](0012-every-new-public-item-shows-an-example.md) | Every public item a change adds shows an example; the rule judges added lines |
| [0013](0013-serve-emits-openapi-from-route-registry.md) | Serve emits OpenAPI 3.1 schema from route registry |
| [0014](0014-std-and-deps-capability-gaps.md) | Capability gaps split between first-party elimination and storefront boundaries |
| [0015](0015-agent-standards-storefront.md) | The storefront sells the agent standards on one ring TLS provider |
| [0016](0016-record-journal-for-a-consumers-own-history.md) | A durable journal over a consumer's own record |
| [0017](0017-a-broker-fence-a-generation-does-not-answer.md) | The broker's fence for what a generation does not answer |
| [0018](0018-a-consumers-fold-on-the-ecs-world.md) | A consumer's fold on the ECS world |
| [0019](0019-a-stop-and-a-place-a-restart-keeps.md) | A stop and a place a restart keeps |
| [0020](0020-what-the-edge-answers-itself.md) | What the edge answers itself |
| [0021](0021-the-kernel-surface-and-a-recorded-instant.md) | The kernel surface and a recorded instant |
| [0022](0022-an-agents-start-page-held-by-a-test.md) | An agent's start page, held by a test |
| [0023](0023-every-page-judged-whole.md) | Every page judged whole |
| [0024](0024-a-view-fed-ahead-of-its-journal.md) | A view fed ahead of its journal |
| [0025](0025-a-command-holds-its-request.md) | A command holds its request |
| [0026](0026-tests-run-on-a-shared-ram-volume.md) | A macOS test on the default directory runs on one shared RAM volume at one slot per core |
| [0027](0027-the-repository-profile-wins-over-the-home-one.md) | The repository's dev profile wins over the home one, scratch apps included |
| [0028](0028-a-cold-run-fits-the-budget.md) | A cold run fits the budget |
| [0029](0029-the-logical-ci-verdict-runs-in-this-repository.md) | The logical_ci verdict runs in this repository |
| [0030](0030-a-loom-model-of-the-v7-generator.md) | A loom model of the v7 generator |
| [0031](0031-a-requirement-is-a-caret-and-the-lock-holds-the-release.md) | A requirement is a caret, and the lock holds the release |
| [0032](0032-the-simulation-lane-reads-listings.md) | The simulation lane reads listings, not archives |
| [0033](0033-the-bot-shards-hash-optimized.md) | The bot shards hash optimized |
| [0034](0034-a-stream-a-key-and-a-tree-stop-read-whole.md) | A stream, a key and a tree stop read whole |
| [0035](0035-a-gate-reads-what-ships.md) | A gate reads what ships |
| [0036](0036-a-moved-lockfile-builds-on-mains-nearest-entry.md) | A moved lockfile builds on main's nearest entry |
| [0037](0037-the-it-group-rows-share-one-dependency-build.md) | The it group rows share one dependency build |
| [0038](0038-advisories-match-as-cargo-reads-a-requirement.md) | Advisories match as Cargo reads a requirement |
| [0039](0039-the-streaming-walk-lists-every-link.md) | The streaming walk lists every link |
| [0040](0040-the-register-keeps-to-the-law-at-120-columns.md) | The register keeps to the law at 120 columns |
| [0041](0041-the-cache-keeps-the-builds-that-set-the-wall.md) | The cache keeps the builds that set the wall |
| [0042](0042-macos-and-windows-run-only-the-platform-gates.md) | macOS and Windows run only the platform gates |
| [0043](0043-an-advisory-match-carries-its-title-and-a-skipped-pin-its-name.md) | An advisory match carries its title and a skipped pin its name |
| [0044](0044-a-record-states-only-what-the-device-proved.md) | A record states only what the device proved |
| [0046](0046-a-per-call-user-agent-is-request-level.md) | A per-call user agent is request level |

New records go at the bottom of that table.
