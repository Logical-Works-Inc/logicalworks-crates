# Adjacent asset inventory: The other repos

Part of [Adjacent asset inventory](estate-asset-inventory.md).

## Bring in now (continued)

### 7. `forge-md-store` (repo `forge-harness`, MIT, archived)

Reducer is the sole owner of lifecycle semantics; the store supplies a
transactional append-only tape, and replay through the reducer decides whether
that tape is coherent. There are deliberately no model calls, no clocks, and no
prompt rendering in it.

Cost to weigh: it pulls a bundled SQLite build, which compiles the database from
source and is the heaviest single dependency in this inventory. It is the reason
this is ranked below `forge-md-runtime` rather than beside it.

## Document as future

Recorded now so the design does not preclude them.

| Asset | Repository | Why deferred |
|---|---|---|
| `keel` gates | `keel` | A mature pure-policy / effectful-adapter split applied across a family of refusal gates, with typed verdicts and effects exiled to an adapter. Transferable as a pattern; the build closure is large and the license needs verification first |
| `wwfd/comply` | `wwfd` | The only shipped refusal gate over prose, requiring evidence to outnumber inference two to one. **Proprietary**, so at most the rule may be restated, never the code |
| `moo-core`, `moo-text`, `moo-blocks` | `moo` | Signed event log, CRDT text layer, and a three-way replay equality check that a bounded replay equals a full fold equals the stored relation. The equality check is the most valuable idea here for a resumable session. Heavy closure: bundled SQLite, an elliptic-curve signature stack, and a pinned cross-repository git dependency |
| `logical-db-episode` | `logical-DB` | Bounded local episode model with fail-closed handling of unknown versions. Apache-2.0. It is itself a port of a model in `LOGICAL-EVENT-MEMORY`, so read the original first |
| `wm-core` | `cairn` and `world-model` | Content addressed by BLAKE3-256, byte-identical to the digest the flow IR uses, with an explicit gate forbidding it from defining its own `Cid`. The naming discipline is the precedent worth inheriting. Note the crate name is taken on the public registry by an unrelated project |

## Nothing found

Two capability areas have no existing implementation anywhere in the
organization, and one is a deliberate exclusion.

- **Element and selector resolution.** No dedicated resolver exists. The only
  real selector work is a set of per-page field maps welded to one specific web
  application and one browser driver. This must be written fresh. What is worth
  taking is the *verdict discipline* rather than the mechanism: a step resolves
  to verified, to unverified, to failed, or to fatal, where unverified carries
  both sides of the uncertainty, and a run passes only when every verdict is
  verified. That discipline exists because a report could not previously
  distinguish "the value reached the form" from "the selector matched nothing".
- **Declarative document compilation.** Nothing beyond the known reference in
  `keel`. There is no second canonicalise-then-validate pipeline.
- **Flow graph as data.** Nothing beyond the known flow IR. A set of four
  `maps-*` crates were examined and excluded: all four declare in their own
  module documentation that they are skeletons emitting no route, no guidance,
  and no runtime behaviour, and they are under a licence this workspace does not
  use.

## Cross-cutting notes

**One hash, not two.** The organization already shares a single hashing
convention: BLAKE3-256 through this workspace's `lgwks_std` hash feature, whose
bytes are identical to the flow IR's content identifier. Four separate
repositories bind to it, and one of them explicitly forbids itself from
redefining the type. Any journal or evidence layer added here must use the same
bytes rather than introducing a second content hash, because two answers to
"what is this artifact" make a provenance receipt impossible to check against
itself.

**Confirm canonical copies before citing.** Two repository pairs are near
duplicates of each other: `cairn` and `world-model` hold the same core crates,
and `keel` exists in several working copies. Establish which is canonical before
any of them is named in workspace documentation.

**One candidate is blocked on licence.** `wwfd/comply` is proprietary, so at most
its rule may be restated and never its code. `forge-harness` was recorded here as
unlicensed in a first pass and is in fact MIT; the correction is above. Verify a
licence by looking for a licence file and the repository metadata, not only by
reading crate manifests, which frequently omit the field.

## What this changes

The specification assumed the session and journal layer had to be written from
nothing. That is half wrong, and the correction has two parts.

`rocco-runtime` already carries the deterministic tick, the bounded journal, the
authorization gate, and replay, under an Apache-2.0 licence with an
already-compatible dependency closure. `lgwks-algorithms` already carries the
similarity scoring primitive. And `forge-md-runtime` carries the model-facing
lifecycle: advisory candidates, mechanical promotion, provenance, a bounded
context snapshot, and an untrusted-data envelope around historical material.

Together those cover capability areas two, three, and five. The workstreams
in flight when this inventory was written were therefore the parts that did not
exist elsewhere: the flow document, its validation, and the invariant register.

The remaining gap, and the one no asset in this inventory closes, is element and
selector resolution.
