#![doc = include_str!("lexicon.md")]
#![cfg_attr(docsrs, feature(doc_cfg))]
//! Capability-gated automation bots built on four verbs: Observe, Evaluate,
//! Execute, Query.
//!
//! The supported interface is `script!`: this page shows the `script` module its
//! expansion calls into and the `script!` and `try_join!` macros, and nothing
//! else. They are plain code, not links, because each exists only under its
//! feature (see the table below) and a link to an absent item breaks the
//! no-default-features documentation build.
//! A consumer hosting flows has one more supported path, [`kernel`], for the clock,
//! journal, broker, lease, gate, effect and retry parts underneath them (#845).
//! The crate root exports `script!`, `try_join!`, `script`, `kernel` and `app`
//! and nothing else (#1016). Every other module is `#[doc(hidden)]` support,
//! named by its full path in this crate's own tests and in the expansion, and
//! kept out of the documented surface by #526.
//!
//! Bots are assembled from `(condition, action)` chains that bind observed
//! sources to side effects. Authority is proof-carrying: every `poll`,
//! `execute_action`, and `query` takes an `(Auth, input)` tuple, and only
//! `GrantSet::issue` can mint the `Auth` half. Capabilities are validated at
//! build time (a bot that requires `bot.net` without a grant fails before it
//! runs) and proven again on every call.
//!
//! That authority is a **snapshot**, not a live lease. `build` clones the grant
//! set into the bot and `GrantSet` has no revoke operation, so changing or
//! dropping the caller's set afterwards does not narrow a bot already built. An
//! `Auth` in hand is likewise a capability-membership proof over the caps it was
//! issued with: not a signature, not an identity, and not isolation. A proof
//! minted from a grant that named a lifetime lapses once that lifetime has
//! passed, but the lifetime is fixed at the mint and a bot mints afresh on
//! every call, so it bounds a proof in hand rather than a running bot. Narrowing
//! a running bot is one operation: `Bot::replace_grants` (under `ecs`)
//! swaps the retained set between ticks, atomically with respect to them, and
//! reports the capabilities it withdrew — which the next tick's calls are denied
//! for. Replacing the caller's set is still not that operation, and the boundary
//! is spelled out under "Authority is a snapshot" in the crate `README.md`.
//!
//! `Evaluate` takes no proof: it is pure (boolean in, boolean out) with no
//! side effect to gate.
//!
//! # Async orchestration runtime
//!
//! `lgwks_bot` is the async orchestration runtime of this workspace: `Runtime`,
//! `rt::task::join_all_bounded`, timers, channels, and the opt-in `net`/`process`/
//! `fs`/`signal` drivers. The driver behind it is sourced from the `lgwks_deps`
//! dependency storefront (`feature = "tokio"`), so no other crate authors a driver
//! edge and no consumer names one. `--no-default-features` is a narrow test build
//! with no driver at all, driving the four verbs on `lgwks_std::task`'s executor
//! instead; it is not the supported surface. The supported surface is the runtime:
//! one front door (`Runtime`), one way to start supervised work (`Supervisor`,
//! `join_all_bounded`, `script!`), and no handle to a running task anywhere.
//!
//! # Features
//!
//! | Feature | Default | Enables |
//! |---|---|---|
//! | `rt` | on | the orchestration runtime surface, driven by the `lgwks_deps` driver |
//! | `time`, `sync`, `macros` | on | the matching runtime drivers; each implies `rt` |
//! | `script` | on | `script!` and the `script` module; implies `rt`, `time`, `sync`, `macros` and `wire` |
//! | `ecs` | on | the `bevy_ecs` substrate `Bot` executes on; opt out with `--no-default-features` for a process-only build |
//! | `trace` | on | the tracing facade; the installer lives in `lgwks_std/trace-install` |
//! | `wire`, `ron` | on | the wire encoding and the RON document notation the journal and the session persist |
//! | `io` | off | reader and writer traits; implies `rt` |
//! | `net`, `fs` | off | sockets, files; each implies `io` (`net` also `lgwks_std/random` and `lgwks_std/http`) |
//! | `process` | off | supervised child processes; implies `io`, `sync` and `time` |
//! | `signal` | off | OS signals; implies `rt` |
//! | `inspect` | off | structural code inspection through `lgwks_ast` (host only) |
//! | `ephemeral` | off | run identities minted from `lgwks_std::random` |
//! | `profile` | off | the per-stage tick profiler |
//! | `sealed` | off | journal and run-store records sealed at rest under fresh data keys (#785) |
//! | `full` | off | every feature above |
//!
//! # Release boundary
//!
//! This page describes the tree the crate was published from. Which release
//! carries a given symbol is in the changelog (`CHANGELOG.md` in the repository,
//! with the 3.0.0 migration notes under `docs/changelog/`); check it before
//! relying on a symbol. Everything outside `script!`, `try_join!`, the
//! `script` module, `app` and [`kernel`] is `#[doc(hidden)]` support code with
//! no compatibility promise (#526).
//!
//! # Quick start
//!
//! A flow is written as indented blocks. Each block is one bounded primitive: a
//! fan-out with a ceiling, a deadline, a retry under a shared budget. The flow
//! below squares every number, retries a failed attempt under the same step key
//! and gives each attempt a second.
//!
//! ```
//! # use lgwks_bot::script::{FlowError, Scope, Tenant};
//! lgwks_bot::script! {
//!     /// Square every number, as many at once as the machine sustains.
//!     pub flow squares(numbers: Vec<u64>) -> u64:
//!         let squared = each number in numbers:
//!             retry up to 3 times, waiting 10ms:
//!                 within 1s:
//!                     number.saturating_mul(number)
//!         give back squared.iter().sum()
//! }
//!
//! # fn main() -> Result<(), FlowError> {
//! // Every flow takes a scope first; it carries the tenant, the step path and
//! // the cancellation token.
//! let scope = Scope::root(Tenant::new("acme")?);
//! let total = lgwks_bot::script::block_on(squares(&scope, vec![1, 2, 3]))?;
//! assert_eq!(total, 14);
//!
//! // The block also emitted a map of what it declared.
//! assert!(ARCHITECTURE.to_string().contains("each number in numbers"));
//! # Ok(())
//! # }
//! ```
//!
//! The same flow is available as ordinary functions: `script::within`,
//! `script::retry` and `script::each` take a `script::Scope` and a body and
//! return a `script::FlowError` that names the step where it failed. Use the
//! macro for the shape and the functions when the body already exists. The
//! blocks need the default `script` feature (see the table above).

// Verbs are async by design (single-threaded `lgwks_std::task` driver, futures
// deliberately not `Send`). The verb traits declare their async methods as
// return-position `impl Future` rather than `async fn`, which is exactly the
// shape INV-BOT-FOUR-VERBS requires without a crate-level lint suppression:
// a `Send` bound would force every domain to be `Send` and rule out the
// thread-local state `Bot::tick` is built for, and implementers still write
// `async fn` in their impls.
//
// Remaining lint contract (missing_docs deny, unsafe_code forbid,
// broken_intra_doc_links deny) comes from the workspace.

/// Compiles every Rust block in the README pages as a doctest.
///
/// The README is the first thing a consumer reads and the last thing anyone
/// updates: it described a `lgwks_std::task` executor and an `async fn tick`
/// for two commits after both had been replaced, and nothing failed, because a
/// prose example is not compiled by anything. This makes it compiled.
///
/// Only `README-script.md` holds a Rust block now: it is the one that goes
/// through `script!`. The examples that named the crate by path became tests
/// (`tests/it/readme_front_door.rs`, `tests/it/rt_front_door.rs`), and
/// `contract/PUBLIC_SURFACE.toml` refuses a README page that gains one.
///
/// `#[cfg(doctest)]` so the item exists only under `cargo test --doc`: it is
/// not part of the library, not built by a normal compile, and not exported.
#[cfg(doctest)]
#[doc = include_str!("../README-script.md")]
struct ReadmeExamples;

/// Compiles every Rust block of the agent skills as a doctest (ADR 0084).
///
/// An agent copies a skill's block and trusts it, so a block no compiler reads
/// teaches an API that may be gone. The `skill-hygiene` lane refuses a skill
/// whose Rust block is not included here.
#[cfg(doctest)]
#[doc = include_str!("../../../skills/lgwks-script-flows/SKILL.md")]
#[doc = include_str!("../../../skills/lgwks-durable-runs/SKILL.md")]
#[doc = include_str!("../../../skills/lgwks-kernel/SKILL.md")]
#[doc = include_str!("../../../skills/lgwks-test-on-the-rails/SKILL.md")]
#[doc = include_str!("../../../skills/lgwks-std-first/SKILL.md")]
struct SkillExamples;

/// The environment broker: which environments a run owns, which generation each
/// is at, and the authority to hand one exact attempt to one of them.
///
/// Fencing lives here rather than in the journal because a durable record of a
/// dispatch aimed at a replaced environment would be accurate and still wrong.
/// See [`broker`] for why a stale generation and a generation the broker
/// never issued are deliberately different errors.
#[doc(hidden)]
pub mod broker;
/// Capability tokens and sealed authority proofs.
#[doc(hidden)]
pub mod cap;
/// Bearer capability tokens with caveats and explainable policy decisions.
#[doc(hidden)]
pub mod cap_token;
/// Versioned shared records: compare-and-set writes for concurrent editors.
///
/// [`CasWrite`](crate::domain::cas::CasWrite) applies a write only when the
/// record still holds the version the writer saw; a late write is the typed
/// [`KernelError::Conflict`](crate::error::KernelError::Conflict) outcome — recorded in the
/// store's bounded conflict log, never retried blindly (issue #278, row 5).
#[doc(hidden)]
pub mod cas;
#[doc(hidden)]
pub mod domain {
    //! Shipped automation domains.
    //!
    //! One path, `lgwks_bot::domain::net`, rather than a second short alias at
    //! the crate root. Nine generic words — `fs`, `net`, `sys`, `data` — read
    //! like the standard library's at a call site that has not imported this
    //! crate's prelude, and a second path is a second thing to keep stable.
    pub mod cas;
    pub mod chat;
    pub mod data;
    pub mod eval;
    pub mod flow;
    pub mod fs;
    pub mod gh;
    /// The structural-inspection domain (`inspect` feature): R8's operation as
    /// an [`Observe`](crate::verb::Observe) source and a
    /// [`Query`](crate::verb::Query), plus a [`Task`](crate::task::Task) a
    /// [`Host`](crate::task::Host) can run.
    #[cfg(feature = "inspect")]
    pub mod inspect;
    /// The network Observe domain, behind `net` like the HTTP exchange it polls
    /// through: a process-only build compiles no HTTP client (#687).
    #[cfg(feature = "net")]
    pub mod net;
    pub mod notify;
    pub mod sys;
}
#[cfg(feature = "edge")]
pub mod app;
/// The app layer's error: a sum of the layer errors. Public under its own path
/// since #1016 took the root re-export away (#990).
#[doc(hidden)]
pub mod app_error;
/// Sequential per-tenant execution cells: the request system's thread-per-core
/// host, its command logs and its effect seam (issue #790). Private: it is the
/// implementation behind [`RequestFeed`](crate::ecs::RequestFeed), not a second
/// way to serve requests. Gated with the edge, its one producer.
#[cfg(feature = "edge")]
mod cell;
/// The one declared clock, which every deadline this crate evaluates names.
///
/// Ungated because the module is `std`-only — it imports `fmt`, `sync::Arc`, an
/// atomic and `time::Instant`, and no engine facade — and it is reached by three
/// surfaces with different gates: `rt::time` re-exports it beside `Deadline`,
/// `rt::supervise` reads it to measure its budgets, and the observation phase
/// reads its wall watchdog to bound a source poll. Gating it on the narrowest of
/// those would leave a `--no-default-features` build — whose tick phase still
/// runs and still needs a bound on its sources — unable to name the clock that
/// governs it.
///
/// Re-exported unchanged from `rt::clock` for every build that has `rt`, so
/// this is one clock reached two ways and never two clocks: `rt::clock` is the
/// path a caller who has the async surface already writes.
#[doc(hidden)]
pub mod clock;
/// The cross-host lease with fencing epochs, the bounded dispatch queue, and
/// the coordinator that commits every dispatch row before the lane starts.
#[doc(hidden)]
pub mod dist_lease;
/// The one durable whole-document replace: write, sync, rename, sync the
/// directory. Private: `cas`, the `tenant_store` snapshot and the journal's seal
/// are its callers, so the order of those calls is decided once (#910).
mod durable;
/// The `bevy_ecs` substrate the verbs execute on, and the one path to the types
/// it reports in, since #1016 took the crate-root copies away. Its substrate
/// submodules are gated behind `ecs` inside, so a process-only consumer never
/// compiles them (#687); the pending-work vocabulary they report in is built
/// everywhere.
#[doc(hidden)]
pub mod ecs;
/// Durable effect identity: the exact key a settlement is a statement about.
///
/// The ledger already refuses a contradicting settlement; this supplies the
/// identity that makes "contradicting" decidable. An attempt count cannot: two
/// deliveries both arriving as "attempt 3" are indistinguishable, so a repeat
/// of an old settlement and a statement about a new one look the same at the
/// moment the ledger accepts one. See [`EffectKey`](effect::EffectKey).
#[doc(hidden)]
pub mod effect;
/// The kernel's typed errors: [`KernelError`](crate::error::KernelError), and the retry vocabulary every
/// layer's error answers in.
#[doc(hidden)]
pub mod error;
/// Politeness and admission: whether a host may be contacted now, and if not,
/// what the scheduler does instead.
///
/// A verdict, not a delay. The gate reports one of three physical actions, and
/// the reasons behind them are typed state machines rather than more arms — see
/// the module documentation for why that is the difference between a schedule
/// that replays and an execution token that became an audit log.
#[doc(hidden)]
pub mod frontier;
/// Grant sets: build-time admission and per-tick proof minting.
#[doc(hidden)]
pub mod gate;
/// One effect per key: POST through a retrying proxy without sending twice.
/// Behind `net` like the HTTP exchange it posts through (#687).
///
/// [`IdempotentPost`](crate::idempotent::IdempotentPost) carries the
/// caller-generated `Idempotency-Key` every attempt of one operation reuses,
/// so a proxy that duplicates and replays requests still delivers exactly one
/// effect. Wired rather than offered: an upstream credential refusal on this
/// path is the credential row's typed repair, and the row's tests stand a
/// real duplicating proxy in front of the adapter (issue #278, row 6).
#[cfg(feature = "net")]
#[doc(hidden)]
pub mod idempotent;
/// Typed in-process structural code inspection (R8, feature `inspect`).
///
/// Check risky code over its structure without executing it: parse the subject
/// bytes with [`lgwks_ast`], walk the tree against a versioned structural rule
/// set, and return a typed [`Verdict`](inspect::Verdict) that distinguishes a
/// complete clean scope from unsupported, undecidable, incomplete and
/// infrastructure-failure outcomes. The subject is never compiled, imported,
/// built, shelled or loaded.
#[cfg(feature = "inspect")]
#[doc(hidden)]
pub mod inspect;
/// The interface model: recognizing the element a step names.
#[doc(hidden)]
pub mod interface;
/// The durable journal an effect is appended to before it leaves the process.
///
/// Identity without persistence is identity that is lost exactly when it is
/// needed, so this is the other half of what [`effect`] supplies. It is
/// also where the durability grade lives: a journal that cannot survive its own
/// writer dying refuses to be the record behind an external handoff, rather
/// than accepting an append it will lose.
#[doc(hidden)]
pub mod journal;
/// JSON through the shared facade (`lgwks_std::json`).
#[doc(hidden)]
pub mod json;
/// The supported surface for a consumer building its own durable runtime (#845).
pub mod kernel;
#[doc(hidden)]
#[cfg(feature = "time")]
pub mod keyed;
/// Language understanding: the tiered lexicon behind the resolver seam.
#[doc(hidden)]
pub mod language;
/// Model output admitted as data: bounded decode, provenance, evidence-gated
/// completion, bounded repair, tenant-scoped artifacts, and a context checkpoint
/// (feature `script`).
///
/// The rule it implements (issue #87): an AI proposal is an **untrusted task
/// input**. Validation, provenance, no-progress detection, bounded repair,
/// tenant-scoped artifacts and serialized writes belong in the host's contract,
/// not in a prompt. So nothing here calls a model — [`proposal::StubModel`] is a
/// deterministic double and the bytes arrive as `&[u8]` from a caller that owns
/// the transport — and nothing a payload says can install a tool, read a
/// credential, widen a grant, or turn a truncated observation into a complete
/// one. Each attempt is a typed [`proposal::Refusal`] carrying the
/// [`proposal::Provenance`] of the bytes that made it.
///
/// It is not a fifth verb: the operations a proposal may name are the ones the
/// host already registered in [`proposal::Surface`], and admitting one produces a
/// [`proposal::Plan`] of names to perform through the existing verbs rather than
/// a new way to perform anything.
///
/// It sits on `script` because the checkpoint round-trips through the run store
/// ([`proposal::Checkpoint`] is [`Durable`](script::run_store::Durable)) and the
/// artifact store is what a task's steps reach.
///
/// # It is wired, and how
///
/// Nothing in this module calls itself: a task body reaches the boundary through
/// [`script::admit`], the one step a run uses to admit model or
/// tool output. A [`Gate`](script::Gate) bundles the decoder, the surface and one
/// **run-scoped** [`proposal::PlanBudget`] and [`proposal::RepairLedger`], and
/// `admit` charges both — so the fourth identical refusal across four separate
/// [`Host::run`](task::Host::run)s is a finite typed
/// [`proposal::Intervention`] rather than four refusals a caller must correlate.
/// It enters its step, so a refusal reads `admit/plan` like every other located
/// failure, and it returns [`FlowError::Refused`](script::FlowError::Refused) or
/// [`FlowError::Intervention`](script::FlowError::Intervention), both typed and
/// both carrying the [`proposal::Provenance`] of the exact refused bytes. When the
/// run has a store installed, each refusal is recorded through it under
/// `<step>/refusal` before the error is returned, so a run resumed on a fresh host
/// reads back what the first run refused. See `INV-BOT-95`.
#[cfg(feature = "script")]
#[doc(hidden)]
pub mod proposal;
/// The `domain_id -> constructor` registry: what a spec's strings resolve to.
///
/// One concern, which identifiers a binary can run, named by its own path since
/// #1016 took the crate-root copies away.
#[doc(hidden)]
pub mod registry;
/// Whether a failed attempt may be tried again.
///
/// The vocabulary lives in [`error`]: [`RetryClass`](crate::error::RetryClass)
/// says what a failure permits and
/// [`DispatchCertainty`](crate::error::DispatchCertainty) says what it established. What this module adds is RQ-009's decision rule, which
/// composes those with the budget, the authority, the intent and any remote
/// deduplication contract.
#[doc(hidden)]
pub mod retry;
/// The canonical PR-review task: pin a subject, publish at that commit, and
/// verify the publication independently.
///
/// [`review::review_pr`] pins the subject, runs the caller's analysis, checks
/// freshness, publishes once at the reviewed commit, and verifies through a
/// separate read-back. Its five outcomes are five facts a caller can act on
/// without reading a message: [`review::ReviewOutcome::Published`] carries a verified
/// review id, [`review::ReviewOutcome::Unknown`] an effect that may or may not have
/// landed, [`review::ReviewOutcome::TargetMoved`] the two commits of a head that
/// changed, and [`review::ReviewOutcome::Refused`] something the caller can repair.
///
/// The body manages no pid, no reap loop and no retry over publication. A lost
/// response is reconciled by exactly one read, never by a second create — and
/// [`review::ReviewOutcome::Unknown`] is what makes the local mode's limit visible
/// rather than hidden: it holds no durable record between attempts, so an
/// effect it cannot observe from GitHub is reported as unknown for the caller
/// to decide.
///
/// # Example
///
/// The task is built and driven against a scripted `gh` in `tests/it/pr_review_journey.rs` and
/// `tests/it/sim_review_pr.rs`. A real run needs a `gh` on `PATH` and a repository to read, so no
/// documentation example reaches the network.
#[cfg(all(feature = "script", feature = "process"))]
#[doc(hidden)]
pub mod review;
/// Async runtime surface (feature `rt`): owned `Runtime`, bounded fan-out,
/// timers, channels, and opt-in drivers.
#[cfg(feature = "rt")]
#[doc(hidden)]
pub mod rt;
/// Orchestration blocks for [`script!`]: scoped, bounded, deadline-carrying,
/// keyed, and mapped (feature `script`).
#[cfg(feature = "script")]
pub mod script;
/// Sealed records at rest (feature `sealed`, #785).
///
/// The encrypted half of the durable story: every record is sealed with
/// ChaCha20-Poly1305 under a fresh data key of its own, wrapped by a
/// [`KeyProvider`](sealed::keys::KeyProvider), and every frame chains the
/// ciphertext with its key identity, so a shredded record still verifies as
/// present-and-erased. Shredding destroys the key rather than the bytes
/// (crypto-erasure, NIST SP 800-88); reads fold a destroyed key to a tombstone,
/// a missing key to an error, and a transient provider fault to a retryable
/// error that neither resurrects nor drops the record.
///
/// Supported through [`kernel`] as `kernel::sealed` since #1016; this path is
/// the support path to the same items.
#[cfg(feature = "sealed")]
#[doc(hidden)]
pub mod sealed;
/// The semantic tier: resolving an utterance a lexicon cannot reach.
#[doc(hidden)]
pub mod semantic;
/// Synchronous validated guidance flows and session runner.
#[doc(hidden)]
pub mod session;
/// The error vocabulary's escaping and retry-class promises, simulated; declared
/// here because `error.rs` is past the size law.
#[cfg(test)]
mod sim_error;
/// The lexicon resolver's verdicts, simulated; declared here beside the other
/// in-crate simulations.
#[cfg(test)]
mod sim_language;
/// The semantic tier behind the lexicon, simulated with a scripted embedder.
#[cfg(test)]
mod sim_semantic;
/// The seeded draw stream the in-module simulations share.
#[cfg(test)]
mod sim_stream;
/// The owned world's simulations and properties (ADR 0055, #1002), declared here because `world` is
/// compiled into the `world-bench` bin by path and a test module inside it would be compiled there too.
#[cfg(all(test, feature = "wire"))]
mod sim_world;
/// Cross-host time: monotonic deadlines, wall timestamps, and the skew bound.
///
/// [`SkewBound`](crate::skew::SkewBound) is how far two honest clocks may disagree;
/// [`MonoDeadline`](crate::skew::MonoDeadline) and [`WallStamp`](crate::skew::WallStamp) keep
/// the deadline a lease carries apart from the timestamp a record shows. Wired
/// rather than offered: [`Auth::check_remote`](crate::cap::Auth::check_remote) judges
/// a proof minted on one clock against a foreign reading within the bound
/// (issue #278, row 2).
#[doc(hidden)]
pub mod skew;
/// The serializable spec contract, the materializer that walks one against a
/// registry, and the builder that assembles bots.
///
/// `Bot::from_spec` (under `ecs`) is the materializer: a [`BotSpec`](spec::BotSpec)
/// plus a `DomainRegistry` plus the caller's [`GrantSet`](gate::GrantSet) produces a runnable
/// `Bot` through the same `assemble`/`build` path a native bot uses, with no
/// second interpreter and no second execution path. It is all-or-nothing: an
/// unmet need is reported as a complete, attributed [`NeedSet`](spec::NeedSet) and nothing is
/// built, and admission validates a registry with a duplicate identifier rather
/// than letting lookup depend on declaration order.
///
/// The builder is not replaced, and it stays the only path that mints
/// authority: grants come from a `GrantSet` the caller holds, never from the
/// spec, so wire data cannot choose what the bot is able to reach. There is
/// deliberately no `bot!` proc-macro: it would drag the `syn` stack into every
/// consumer and hide the per-call `Auth::check` that auditors read.
/// [`domains!`](crate::domains) declares the registry as data instead, with no
/// proc macro and no new dependency. Orchestration *between* verbs is a
/// different job, and it does have a proc macro: `script!` (feature `script`)
/// expands to plain calls into the `script` module, leaves every verb's
/// `Auth::check` where it was, and reaches `syn` only on the host through the
/// `lgwks_deps` storefront.
#[doc(hidden)]
pub mod spec;
/// The stable-read protocol: what an `Observe` adapter must establish before a
/// reading is allowed to count as a change.
///
/// A subject being written while it is read is not a different subject, and the
/// value a reader gets out of that is one nobody on the system ever held. This
/// module is the discipline commercial automation applies to it with: two
/// independent reads must agree, and a reading that cannot be confirmed is
/// reported as `EcsError::UnstableObservation`: pending, never a change.
///
/// Wired rather than offered: [`domain::data::JsonStore`] reads through
/// [`stability::read_stable_file`], so every poll of a JSON-backed store is
/// confirmed before it is reported.
#[doc(hidden)]
pub mod stability;
/// The task front door: a [`Host`](task::Host) runs a
/// [`Task`](task::Task) and returns a [`Report`](task::Report) (feature
/// `script`).
///
/// The smallest complete path from "here is a typed body" to "here is a
/// disposition, an output and evidence": no trait to implement per workflow, no
/// observer to poll, no handle to reap, and no `Send`, `Box` or `'static` bound
/// in the author's code. It sits on `script`, so a task body composes with
/// `each`, `within` and `retry` and inherits their bounds.
#[cfg(feature = "script")]
#[doc(hidden)]
pub mod task;
/// The estate-owned durable keyed store behind multi-tenant run records:
/// tenant policies, run rows and lane rows with per-tenant keyed reads. Every
/// row persists through the wire encoding, so the store exists with `wire`.
#[cfg(feature = "wire")]
#[doc(hidden)]
pub mod tenant_store;
/// The four verbs: Observe, Evaluate, Execute, Query. No fifth verb exists.
#[doc(hidden)]
pub mod verb;
/// The owned world: one tenant's columns, change ticks, transactions and snapshots (ADR 0055, #1002).
///
/// Private and compiled under test only. Until the cell holds a world (#1004) nothing in the shipped
/// library calls it, and a private module nothing calls is dead code. The `world-bench` bin includes the
/// same files by path and calls every item, and the simulations in `sim_world` call them under test.
/// #1004 removes the `cfg` and the include together.
#[cfg(all(test, feature = "wire"))]
mod world;

/// Write orchestration as indented blocks that expand to bounded,
/// deadline-carrying, tenant-keyed flows over [`script`](mod@script), plus an
/// `ARCHITECTURE` map of what was declared. The language is documented on
/// the [`script`](mod@script) module and in `lgwks_macros`.
///
/// # Examples
///
/// A flow that sizes every page with a second per step. A limit that comes from
/// outside, such as an upstream's quota, is written `at most (quota) at once`;
/// a literal there is refused at compile time, because it is right on one host
/// and wrong on the next.
///
/// ```
/// # use lgwks_bot::script::{FlowError, Scope, Tenant};
/// lgwks_bot::script! {
///     pub flow total(pages: Vec<u64>) -> u64:
///         let sizes = each page in pages:
///             within 1s:
///                 page.saturating_add(1)
///         give back sizes.iter().sum()
/// }
///
/// # fn main() -> Result<(), FlowError> {
/// let scope = Scope::root(Tenant::new("acme")?);
/// assert_eq!(lgwks_bot::script::block_on(total(&scope, vec![1, 2, 3]))?, 9);
/// # Ok(())
/// # }
/// ```
#[cfg(feature = "script")]
pub use lgwks_macros::script;

/// Wait for all of a set of fallible futures, short-circuiting on the first
/// error. Re-exported from the crate-owned driver so callers never name it.
///
/// Every branch is polled on the task that awaits the macro, so the branches may
/// borrow the caller's locals. The result is `Ok` with a tuple of the branches'
/// values in the order written when every branch succeeds, or the first `Err`
/// any branch returns, which drops the branches still running. In `script!` it
/// is the `together:` block.
///
/// # Examples
///
/// ```
/// # use lgwks_bot::script::{FlowError, Scope, Tenant};
/// lgwks_bot::script! {
///     pub flow pair(n: u64) -> u64:
///         together:
///             let doubled = n.saturating_mul(2)
///             let tripled = n.saturating_mul(3)
///         give back doubled.saturating_add(tripled)
/// }
///
/// # fn main() -> Result<(), FlowError> {
/// let scope = Scope::root(Tenant::new("acme")?);
/// assert_eq!(lgwks_bot::script::block_on(pair(&scope, 10))?, 50);
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "rt", feature = "macros"))]
pub use lgwks_deps::tokio::try_join;
