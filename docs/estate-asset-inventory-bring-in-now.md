# Adjacent asset inventory: Bring in now

Part of [Adjacent asset inventory](estate-asset-inventory.md).

## Bring in now

Ranked. Each of these is licensed compatibly, small in dependency closure, and
closer to the requirement than anything in this workspace.

### 1. `rocco-runtime` (repo `Rocco`, Apache-2.0)

The lowest invariant, made executable: a deterministic tick, a bounded journal,
an admission and authorization gate, an effect lifecycle, and replay. Three
externally falsifiable invariants are declared:

- **The journal is authoritative.** State is a fold over recorded facts, and
  `runtime::replay` rebuilds a projection from a journal alone, consulting no
  live state.
- **The journal is bounded.** The append that would exceed the ceiling is
  refused, and the refusal is backpressure rather than loss.
- **Authorization separates two clocks.** A pure reducer records the decision
  with the intent; a `RevocationFence` carries live freshness, on the stated
  ground that a recorded decision from yesterday is exactly the evidence that
  must not be trusted today.

Dependency closure is `rocco-contracts` plus `lgwks_std`, both already in this
organization. No network, no database, no async runtime.

**Consequence for the specification:** the interim `Journal` seam, backed by an
in-memory vector, is weaker than this. Journal-authoritative replay is the
correct target and should replace the interim implementation rather than sit
beside it.

### 2. `xpress-runner` content store and write-ahead log (MIT)

A content-addressed store in which an artifact's identity **is** the BLAKE3 of
its bytes, paired with a checksummed append-only log. The log's discipline is
the point: a browser action is announced in it *before* the driver is called,
and the outcome is appended *before* any in-memory projection or observer may
announce it. A clean incomplete log therefore means "outcome unknown", never
"the field was not mapped".

That is the evidence invariant this specification wants, already running.

### 3. `xpress-runner` browser session, guard, and access modules (MIT)

Three separable seams:

- `session.rs` classifies where a session is and what the runner may do about
  it, and keeps classification separate from action. It names the exact URL a
  human should open and types no credential.
- `guard.rs` is a fail-closed authority recogniser. It replaced a substring host
  check that accepted a hostile URL containing the permitted host as a query
  parameter. It recognises exactly one shape and refuses explicit ports,
  userinfo, percent-escapes, backslashes, non-ASCII, and control characters
  rather than normalising, choosing the direction that fails safe.
- `access.rs` models the signed-in user's access level as the scope they may
  operate in, enforced separately from any form field.

### 4. `lgwks-algorithms` graph and dedupe modules (MIT OR Apache-2.0)

Dependency-free building blocks: path-halved union-find with union by size,
minimum-spanning-forest construction, a brute-force k-nearest-neighbour graph in
compressed sparse row form with symmetrisation policies, SimHash random
projection, and MinHash near-duplicate detection.

`KnnGraph` computes cosine similarity and Euclidean distance, which is precisely
the scoring ground the companion specification's similarity workstream covers.
The estate already has this; the specification should consume it rather than
grow a second implementation.

### 5. `lgwks-algorithms` declarative module (MIT OR Apache-2.0)

Encapsulates algorithms in pure pipelines over `(Security, Capability,
Justification)` tuples, and states the boundary explicitly: the values are
diagnostic metadata, not authority, so `execute_declarative` refuses the critical
execution level unconditionally. This is the cleanest existing statement in the
organization of an invariant register whose refusal is unconditional rather than
configurable, which is the shape the companion specification's invariant register
should copy.

### 6. `forge-md-runtime` (repo `forge-harness`, MIT, archived)

A durable memory lifecycle runtime, and the strongest match in the organization
for the model-facing half of this work. It is substantially more than a journal:
it owns the deterministic part of a task lifecycle, journals the exact
task and transcript bytes, derives a source-addressable episode record, and
builds a bounded context snapshot before any successor runs.

The property that matters most: **model-generated candidates stay advisory.**
Candidates are proposed, evaluated, and quarantined through distinct operations,
and only a mechanically derived record is promoted. A model's claim is never
itself the record.

Other properties worth taking:

- A bounded context snapshot with an explicit token budget, rather than an
  unbounded history.
- A retention policy with an enforced eviction path.
- An HMAC-authenticated append-only event log with an issuer identity, so a
  recorded event carries who wrote it.
- A context envelope whose opening marker states that the historical material
  inside is escaped untrusted reference data and not instructions, permissions,
  or authority. That is a prompt-injection boundary expressed as a first-class
  constant rather than a convention.

This repository is **archived and MIT licensed** (`sdk/LICENSE`; the GitHub
repository metadata reports `mit`). An earlier pass recorded it as unlicensed
because it read the crate manifests, which declare no `license` field, and did
not look for the file. That was wrong.
