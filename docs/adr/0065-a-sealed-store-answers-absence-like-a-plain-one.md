# ADR 0065: a sealed store answers absence like a plain one

Status: **accepted**, 2026-11-18. Implements #1085.

Stakeholders: the Director; a consumer that hosts durable flows over a sealed run store; a reviewer listing approvals.
Concerns: a Host over a sealed store failed its first step; a key fault dropped parked approvals from the listing.
Requirements: R10
Viewpoint: the `RunRecords` contract, as a host reads a step.
Elements and relations: `RunStore::lookup` and `pending_scan` in `task::store`; `pending_sealed_value`; `pending_for`.

## Context

ADR 0009 parks a run for a human decision, and the sealed store of #785 encrypts every step. The two never met. On a
sealed store, `lookup` of a step never recorded answered `StoreError::SealedUnknown`, not `Ok(None)`. A host reads
each step before it runs it, so every flow failed at its first step: `hold/hold/approval` refused `SealedUnknown`.
`pending_scan` then folded every provider refusal, transient included, to "not there", so a short key fault listed
fewer approvals and returned no error.

## Decision

1. **A sealed `lookup` of a step never recorded is `Ok(None)`**, as on a plain store. A shredded slot is refused as
   `SealedErased` before this answer, and a destroyed key folds to erased, so absence never hides an erasure.
2. **A sealed pending step that does not open fails the listing.** `pending_sealed_value` returns
   `Result<Option<_>, StoreError>`: erased is `Ok(None)`, a refusal is `StoreError::SealedKey`. `pending_scan` and
   `pending_for` pass it up, and `Host::pending` returns it as `FlowError::Store`.

## Alternatives rejected

- **Keep `SealedUnknown` for an unknown step.** It holds a torn tail to a refusal, but no host runs a flow on it.
  The empty store refuses too.
- **Skip a pending step that does not open, and log it.** A reviewer sees a shorter list and no error.
- **Retry inside the scan.** A long fault still drops steps or stalls the listing, and the caller cannot shed it.

## Consequences

- A torn tail that took a sealed step reads as absent, and the host runs the step again, as on a plain store.
  Reverse this if a test shows a sealed step that read absent after it was durably appended and synced.
- A listing fails for the length of a key fault. The caller retries on the typed transient arm.

## Correspondence

Relates to ADR 0009 (approvals park the run) and the sealed store of #785.

## Verification

`tests/it/sealed_pending_fault.rs`: a transient fault fails the listing typed, then lists the approval again; one
fault hides no tenant's approval; a reopened host lists the earlier park; a fault in a 64-approval listing fails it
whole. `sealed::a_torn_shred_leaves_live_or_erased_never_readable_without_a_key` holds decision 1 under tears.
