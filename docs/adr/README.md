# Architecture decision records

Every architecture decision in this repository is a record here, and the record
is held to **ISO/IEC/IEEE 42010** by `check-docs` in the `doc-governance` lane.
Prose review does not catch a missing field; a refusal does.

| Field | 42010 clause | What it must say |
|---|---|---|
| `Status:` | 5.2: an AD identifies its state | `proposed`, `accepted`, `superseded`, `rejected` or `deprecated`, and the `YYYY-MM-DD` it was decided |
| `Stakeholders:` | 5.1: stakeholders | who holds a concern about this decision |
| `Concerns:` | 5.1: concerns | the interest in the system this record answers |
| `Requirements:` | 6.5: rationale | the `REQUIREMENTS.md` ids (`R1`) the decision serves; a decision that traces to nothing is not one |
| `Viewpoint:` | 5.2: viewpoints | the model kind and conventions the view is written in |
| `Elements and relations:` | 5.2: views | the elements (crates, modules, features) and the relations between them the record describes |
| `## Context` | 6.5: rationale | the situation the decision answers |
| `## Decision` | 6.5: rationale | what was decided, in the imperative |
| `## Alternatives rejected` | 8.2: rationale | each credible alternative, why it lost, and what it cost |
| `## Consequences` | 8.2: rationale | what the decision costs, including what stays open |
| `## Correspondence` | 7.3: correspondence | the other records this one supersedes, refines or relates to |
| `## Verification` | 8.3: conformance | the test, gate or lane that holds the decision true, or `review only` with its owner |

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
| [0006](0006-one-ecs-world-one-tokio-axum-driver.md) | One ECS world is the core; one driver on tokio and axum carries events in and effects out (decision 1 refined by 0053) |
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
| [0045](0045-a-run-carries-its-authority-on-its-scope.md) | A run carries its authority on its scope |
| [0046](0046-a-per-call-user-agent-is-request-level.md) | A per-call user agent is request level |
| [0047](0047-a-capability-open-never-blocks-and-a-listing-owns-its-offset.md) | A capability open never blocks, and a listing owns its offset |
| [0048](0048-a-shallower-path-reenters-a-depth-cut-directory.md) | A shallower path re-enters a depth-cut directory |
| [0049](0049-a-run-carries-its-store-definition-and-effect-policy-on-its-scope.md) | A run carries its store, definition and effect policy on its scope |
| [0050](0050-request-options-debug-names-headers-without-values.md) | Request options Debug names headers without values |
| [0051](0051-a-cited-invariant-id-is-a-registered-one.md) | A cited invariant id is a registered one |
| [0052](0052-the-runtime-is-lgwks-runtime-in-seven-crates.md) | The runtime is `lgwks_runtime` in seven crates under one facade |
| [0053](0053-a-world-per-tenant-on-an-owned-engine.md) | A world per tenant on an owned engine; the cell host is the supervisor |
| [0054](0054-each-tenant-spends-its-own-effect-budget.md) | Each tenant spends its own effect budget |
| [0055](0055-the-owned-world-is-columns-ticks-and-one-hash.md) | The owned world is columns, ticks and one hash (refines 0053) |
| [0056](0056-a-served-run-is-owned-by-the-serve-not-the-connection.md) | A served run is owned by the serve, not the connection |
| [0057](0057-the-serve-shards-its-tenant-hosts-and-caps-them.md) | The serve shards its tenant hosts and caps them |
| [0058](0058-drishti-has-one-home.md) | drishti has one home: the Canvas code folds into `crates/drishti` and reaches every third-party crate through the storefront |
| [0059](0059-the-serve-builds-each-route-task-once-at-start.md) | The serve builds each route's task once, at start |
| [0060](0060-the-serve-keeps-its-request-hashes-and-pins-their-bytes.md) | The serve keeps its request hashes and pins their bytes |
| [0061](0061-one-typed-error-per-layer.md) | One typed error per layer; a lower layer never names an upper one |
| [0062](0062-test-builds-of-workspace-crates-are-incremental.md) | Test builds of workspace crates are incremental |
| [0063](0063-the-durable-bench-rival-is-restate-in-the-bench-workspace.md) | The durable bench rival is Restate, in the bench workspace |
| [0064](0064-the-crate-root-exports-the-supported-surface.md) | The crate root exports the supported surface; `kernel` gains `task`, `cap` and `sealed` (refines 0021) |
| [0065](0065-a-sealed-store-answers-absence-like-a-plain-one.md) | A sealed store answers absence like a plain one; a key fault fails the pending listing |
| [0066](0066-ci-builds-on-nightly-releases-build-on-stable.md) | CI builds on a dated nightly, releases build on stable |
| [0067](0067-a-capability-scope-refuses-a-traversal-segment.md) | A capability scope refuses a traversal segment; a denial ends at its rule |
| [0068](0068-a-glob-refusal-names-its-utf-8-byte-offset.md) | A glob refusal names its UTF-8 byte offset |
| [0069](0069-every-hosted-toolchain-carries-the-wrapper.md) | Every hosted toolchain carries the clippy wrapper the cargo config names |
| [0070](0070-a-model-call-is-a-durable-step.md) | A model call is a durable step: a resume replays the answer and never pays twice |
| [0071](0071-a-justify-token-rule-bounds-a-runs-model-spend.md) | A `justify` token rule bounds a run's model spend: reserve before the call, settle to the bill |
| [0072](0072-a-landlock-allowlist-holds-4096-rules.md) | A Landlock allowlist holds 4,096 rules and 1 MiB of path bytes, refused before the fork |
| [0073](0073-gpui-builds-from-zed-main-until-it-ships.md) | gpui builds from Zed main until crates.io holds the fix for two future-incompatible crates |
| [0074](0074-a-moved-lockfile-builds-on-mains-entry-for-its-own-build.md) | A moved lockfile builds on main's entry for its own build (refines 0036) |
| [0075](0075-every-page-reads-zero-findings-with-no-exemption.md) | Every page reads 0 `check-docs` findings, `AGENTS.md` included |
| [0076](0076-doc-tests-run-beside-the-suites-on-their-own-cache.md) | Doc tests run beside the suites, on their own cache |
| [0077](0077-supervised-children-get-a-private-temp-dir.md) | Supervised children get a private temp dir |
| [0078](0078-every-test-is-judged-by-what-it-leaves.md) | Every test is judged by what it leaves |
| [0079](0079-the-shared-darwin-temp-dir-is-scanned-once-per-machine.md) | The shared Darwin temp dir is scanned once per machine |
| [0080](0080-a-finished-run-returns-the-heap.md) | A finished run returns the heap to its baseline |
| [0081](0081-a-parked-child-is-reaped-with-no-later-start.md) | A parked child is reaped with no later start |
