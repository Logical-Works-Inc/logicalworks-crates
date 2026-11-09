# Guidance runner specification: Workstream A

Part of [Guidance runner specification](guidance-runner-spec.md).

## Workstream A: `lgwks_std` similarity primitives

New module `crates/lgwks-std/src/similarity.rs`. Pure functions and one trait.
No I/O, no clock, no allocation beyond the input.

Delivered:

- `trait Similarity` with an associated `Value` type and
  `fn score(&self, left: &Self::Value, right: &Self::Value) -> f64`, returning a
  value in `[0.0, 1.0]`.
- `EditDistance`: bounded Levenshtein over normalized strings, normalized to
  `[0,1]` by the longer input length.
- `Jaccard`: set similarity over attribute collections.
- `PathSimilarity`: normalized structural path comparison, discarding volatile
  segments (numeric ids, generated class hashes) before comparison.
- `Geometry`: Euclidean distance over normalized bounding boxes, mapped to
  `[0,1]` by a caller-supplied maximum.
- `Weighted`: a combinator holding `(weight, boxed Similarity)` pairs producing
  `sum(w_i * phi_i)`, with a `threshold` above which the result is accepted.

Requirements:

- `EditDistance` must be bounded: refuse inputs over a declared maximum length
  rather than allocating a quadratic table for a hostile input.
- `Weighted` must be constructible only with a non-empty set of vectors; an
  empty scorer is a `Result` error, not a silent zero.
- Every public item carries a doc comment. The workspace denies `missing_docs`.
- Zero new dependencies. This module uses only `core` and `alloc`.

Acceptance: unit tests per metric including boundary inputs (empty strings,
disjoint sets, identical inputs, maximum length), a test that a `Weighted`
combinator with a single vector returns that vector's score, and a property test
that every metric returns within `[0.0, 1.0]`.

## Workstream B: `lgwks_bot` session and flow runner

New module `crates/lgwks-bot/src/session.rs` plus `crates/lgwks-bot/src/session/`
for its parts. This is a new top-level module. It does not modify
`domain::flow`, which remains the linear composition domain and is a different
job.

Delivered:

- `NodeKind`: `Say { text }`, `Ask { var, options, routes }`,
  `Branch { var, when, then, otherwise }`, `Handoff { target }`,
  `Refer { target, text }`, `Route { dispatch, fallback }`, `End`.
- `FlowSpec`: declared `vars`, an `entry` node id, a node map, and terminals.
  Constructed by `FlowSpec::from_json` (through `lgwks_std::json`, matching the
  `BotSpec` precedent) and validated at construction.
- `VarScope`: declared, named variables, typed as a closed enum of string,
  integer, boolean, and choice. `${name}` interpolation into `Say` and `Refer`
  text, through the `Interpolate` seam. An undeclared name in a template is a
  validation error at load, not an empty substitution at run time.
- `Session`: holds an id, the current `(flow, node)` cursor, the `VarScope`, the
  visited path, and the transcript. `Session::answer`, `Session::current`,
  `Session::transcript`, `Session::terminal`.
- `Terminal`: `Completed`, `Referred { target }`, `HandedOff { target }`,
  `Refused { reason }`. The outcome of a session is this type, never a bare
  string.
- `trait Resolver`: maps a free-text utterance and a set of candidate options to
  `Option<usize>`. The shipped implementation is a weighted keyword scorer using
  `KeywordResolver`, with an explicit unrecognized case that re-asks the same
  node rather than advancing. A model-backed resolver is the intended future
  `impl`.

  **Superseded, 2026-09-21.** `Option<usize>` was the defect the brittleness
  sweep predicted: it reports an answer that fits two options equally well as
  *unrecognized*, so the session re-asks the full list and the identical answer
  resolves identically forever. The delivered seam returns `Resolution` —
  `Resolved` / `Ambiguous` / `Absent` — and an ambiguous re-ask is narrowed to
  the options still in play. `KeywordResolver` was deleted rather than kept
  beside its replacement, which is a strict superset. See
  `docs/general-bot-fold.md` §3.5.
- `trait Journal`: receives `(path_node, role, text)` records. The shipped
  implementation is an in-memory `Vec`. A durable, sealed implementation is the
  intended future `impl`.
- `flow::validate`: refuses at load, each with a distinct typed error:
  - a transition target that is not a declared node
  - an `Ask` whose option value has no route
  - a declared variable never written
  - a variable read in a template but never declared
  - a node unreachable from `entry`
  - a step count over a declared `budget`
  - an unknown node kind
- A loop guard: a session exceeding `budget` steps returns
  `BotError::SessionBudgetExceeded` rather than looping.

Requirements:

- `Session` fields are private with accessors. The workspace denies
  invariant-breaking `pub`.
- No production `unwrap` or `expect`. Every failure path returns `BotError`.
- No `println!`. The workspace forbids terminal output in library code.
- The runner is synchronous. It drives no I/O and awaits nothing, so it runs
  under `--no-default-features` with no async runtime present. This is
  deliberate: a CDP-driven consumer is synchronous and cannot take the async
  verb surface.

Acceptance is a flow fixture of at least twenty nodes exercising every `NodeKind`,
a scripted session that reaches each terminal kind, and one test per validation
error above. A further test shows an unrecognized answer re-asks rather than advancing,
and another that a cycle exceeding the budget returns
`SessionBudgetExceeded` rather than hanging.
