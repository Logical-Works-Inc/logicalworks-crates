# ADR 0001: Scope, audit and bound the native host broker

## Status

Accepted. Decision ledger entries D18 and D19 in `docs/decisions.md` hold the full text. This record kept its number when the private repository folded into this crate (workspace ADR 0057).

## Context

The broker let a caller name its own scope. A tenant could read another tenant's subscriptions. The command bar needed no grant. Decisions left no trace. Registries had no bound, so one tenant could fill them. Issues drishti#8, drishti#9 and drishti#10 ask for scoped reads, a grant check and an audit trail.

`lgwks_ast` 6 dropped its `Error` derive. The estate owner of that derive is `lgwks_macros`.

## Decision

- Every inspector reads through one `Visibility` filter. The scope comes from the broker's own registration, found by the handle nonce. The handle's own fields are never trusted.
- `Reach::AllTenants` needs the `CrossTenantInspect` grant from an injected grant store. Without it the broker refuses with `CrossTenantDenied`.
- The command bar checks `intent.emit` first and refuses with `CapabilityError::NotGranted`.
- Every decision becomes one typed `AuditEvent` with a sequence number. The broker cannot be built without an audit sink. The sink runs outside the state lock.
- Every registry has a numeric bound and a typed `LimitExceeded` refusal. Widget ids and rate buckets carry the tenant.
- The `api_drive_host` binary drives this chain. It prints each `AuditEvent` as one JSON line, reads the own-scope inspectors, and reads the governance tape under `MAX_RECENT_ROWS`.
- The estate crate `lgwks_macros` 2.1 supplies the `Error` derive.

## Alternatives rejected

- A caller-supplied session id: the requester names its own scope.
- An admin see-all exemption: one stolen credential reads every tenant.
- A no-op audit default: a missing sink would pass in silence.
- A third-party `thiserror`: the law forbids a new third-party dependency.

## Consequences

- A wide read without the grant fails loud and is audited.
- Past a bound the broker refuses and leaves its state untouched.
- Durable audit storage, sink backpressure and a total order across sinks stay open. The sequence number gives the order.

## Verification

- `sim_broker_neighbour_same_session_never_reads_across_tenants` gives two tenants one session id.
- `sim_broker_giant_and_empty_every_bound_refuses_at_limit_plus_one` drives every bound.
- `sim_api_drive_host_journey_is_audited_and_replayable` runs the shipped binary twice and compares the output.
