# ADR 0009: a human decision parks the run

Status: **accepted**, 2026-11-01. Refines ADR 0008 (every capability is an SDK, an entrypoint and a word).

Stakeholders: the Director; the agent adding a capability; a flow author who writes only `script!`; the operator who decides parked approvals on the estate CLI
Concerns: an effect that leaves the process running without the human decision it owes; a run held open by a wait nobody bounded
Requirements: R1, R2, R3, R7
Viewpoint: the vocabulary viewpoint — one row per runtime capability: its word, entrypoint and evidence
Elements and relations: `lgwks_bot::script::approval` and `task::Host::{decide, pending, audit}`; `lgwks_ast::script::LEXICON`, `lgwks_ast::script::parse`, `lgwks_macros::emit`; `word lowers to entrypoint`; the estate CLI drives `Host::decide` over the file store

## Context

The Director, 2026-10-20, on external effects: they are almost always acting on something else, so
refusing them left the `act` word with nothing to do. An action that runs ungated is therefore the
default the estate must close: a flow that pages oncall runs the page the moment it reaches the
line, with no decision owed by anyone, and no record of whose authority released it.

Read from the code that day: `act` checked capabilities and durability and then ran, and no word
parked a run for a human. The pending decision had no durable shape, no id, no expiry and no
second door: an operator could not list what waited, approve or deny it, or learn that a foreign
tenant had tried.

## Decision

1. One word is added, with the SDK, the entrypoint and the evidence ADR 0008 demands:

| Word | Entrypoint | Capability |
|---|---|---|
| `await approval <label> within <duration>:` | `approval_at` | the parked decision: the block's bytes parked as a payload, bound as `Approved` once decided |

2. The word parks without holding a task, thread or timer: the first run commits its pending
   record and ends `Parked`; a resume finds the decision (approved, denied and expired are each
   typed) or parks again under the same id.
3. `Host::decide` records the decision and `Host::pending` lists what waits; both drive the real
   file store, so a restart loses nothing. A decision against other bytes is `DigestMismatch`,
   a second decision is `AlreadyDecided` (the store's own append answer arbitrates the race), a
   foreign tenant is refused and audited, and a lapse is recorded as an expiry decision.
4. `act` is gated by an `EffectPolicy`: the default owes a human decision for every action whose
   effect leaves the process (`EffectLifetime::External`) and refuses without a held approval
   whose digest matches the archived input. The estate CLI (`estate approvals list|approve|deny`)
   drives the same decide path as a flow's park.
5. The ceiling is 10,000 undecided approvals per tenant by default
   (`HostBuilder::max_pending_approvals` lowers or raises it, at most 65,536); a park past it is
   refused at admission, before its payload block runs. Expiry is the `within` the word declares,
   measured on the host's clock, never wall time.

## Alternatives rejected

A timer that waits for the decision inside the run: the run would hold its task, its permit and
its deadline for a human latency, and a restart would lose the wait. Cost: admission budget spent
on waiting, and no durable id to decide. Parking ends the run instead.

A decision by message rather than by record: an approval delivered over a channel has no id a
restart can re-read and no digest a resume can match, so a decision could release bytes the park
never held. Cost: the binding between the decision and its bytes, which is the whole of the
claim. The record carries the digest, and the decision names it.

Leaving the default policy permissive and gating by annotation: every existing external action
would run ungated until its author opted in, which is the open default this record closes. Cost:
the opposite fail direction. A test that drives a stub installs its own policy explicitly, so
the suite never leans on the default to run an effect.

## Consequences

Flows park for a human by word, with refusals at expansion and keys, sites and trail entries
like every other step. The lexicon is 25 words; the pages repaginate to five, still inside the
4,000-token budget and the 120-line law. `lgwks_ast` gains `Kind::Approval`, `Approval` and the
`await` spelling with its refusal case; `lgwks_bot` gains `approval_at`, `StepKind::Approval`,
`Host::{decide, pending, audit}` and `HostBuilder::max_pending_approvals`. Open: an approval the
provider answers while the run is still parked does not wake it; the app feeds the answer to its
`on` handler when the wait is still open, and a later `decide` settles the record.

## Correspondence

Refines ADR 0008 (script as the orchestration layer, every capability a word or named with why
not). Related: ADR 0003 (every capability on by default), #526 (the supported surface is
`script!`), #784 (the order that asked for the word).

## Verification

`lgwks_macros`: `every_refusal_names_its_replacement` (the `approval with no deadline` case),
`the_structured_words_expand_to_their_runtime_calls`, `every_guarantee_names_a_test_that_exists`,
`the_lexicon_page_is_rendered_from_the_lexicon` (token budget), and the README and front-page
table tests. `lgwks_bot`: `tests/it/approval.rs` (park, restart, mismatch, foreign tenant with
audit, lapse, ceiling, gate, match, race, no store and ephemeral store) and
`tests/it/sim_approval.rs` (`parked_runs_resume_after_a_decision` and the replay/divergence
pair). `lgwks_ast`: the grammar band draws the word (`sim_one_band_says_every_word`).
