# Requirements D. Correctness

Part of [the product requirements](../../REQUIREMENTS.md), which state how a requirement changes.

## D. Correctness

### R9. Authority is proof-carrying at the invocation.

> No effect MUST fire without a grant minted by the one issuer, carried on the
> call and checked on the call.

**Invariants.** One mint site. A grant is a value, not a configuration lookup.
A wider grant fires a superset and never a different set. Ambiguity is refused
at build rather than misordered at run time.

**Callers.** Every effect.

**Falsifier.** `T4` in `proofs/`: an entry that fires without the grant that
permits it, for any grant, any environment, any entry list.

**Status.** `met` at the design level, **proved for the model and argued for
the implementation**. `proofs/README.md` states the refinement gap plainly: the
theorems are about the abstract model, and nothing proves `spec.rs` implements
it. `tests/it/authority.rs` covers the Rust surface.

### R10. The record decides what happened.

> After a loss, absent, contradicted, superseded and unreadable MUST be four
> distinct answers. An unknown outcome MUST be a barrier and MUST NOT be
> auto-replayed. A read failure MUST NOT be reported as absence.

**Invariants.** One fold derives attempt status; no second authority derives a
second answer. A journal error propagates as refusal. A settled outcome is not
overwritten by different evidence. An external handoff is refused before the
effect leaves when the record cannot outlive the process.

**Callers.** Every effect that leaves the process.

**Falsifier.**
`tests/it/durable_dispatch.rs::a_transient_journal_read_failure_is_not_reported_as_no_such_work`
and
`tests/it/durable_dispatch.rs::a_reconstructed_bot_retires_an_unchanged_state_rather_than_firing_again`.
Either regressing refutes R10.

**Status.** `partial`, measured against an in-memory journal and — for the five
journal-ladder observations, #100 #101 #102 #104 and #106 — measured against a
real file-backed store under a real process kill
(`tests/it/durable_crash_observation.rs`). Issue
[#109](https://github.com/Logical-Works-Inc/logicalworks-crates/issues/109) closed on
2026-09-23. Of its other three rows, #99 and #108 have no external observation, and
the T21 row of #107 is observed on Unix (`tests/it/process_escape.rs`).

### R11. Bounds are declared at construction.

> Every queue, channel, fan-out, loop and retry MUST have a bound stated where
> it is built. Work MUST NOT be fire-and-forget. A loop MUST NOT spin without a
> bound or a cancellation branch.

**Invariants.** Capacity and overflow strategy at construction. Every spawn
tracked in a supervisor or `JoinSet` with a `CancellationToken`. No `unsafe`.
No production `.unwrap()` or `.expect()`.

**Callers.** Every background unit.

**Falsifier.** A channel with no capacity, a spawn with no owner, a loop with
no exit. In this workspace that is also a hard compile failure: `forbid`, not
`deny`, so a single `#[allow]` anywhere is `E0453` rather than silence.

**Status.** `met` as a compile-time contract, measured. The lint table is in
the workspace manifest and the anti-pattern coverage ceiling is published in
`CODEBOOK.md` rather than overclaimed. `lgwks_std` is the one named exception to
`forbid` for `unsafe` (INV-STD-UNSAFE-1), recorded in the Supersession log.
