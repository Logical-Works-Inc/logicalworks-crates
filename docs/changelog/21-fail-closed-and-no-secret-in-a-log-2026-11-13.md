# lgwks_std 4.2.0 / lgwks_bot 8.4.0

Released 2026-11-13. A refusal now fails closed, a run keeps its own state, and no header value reaches a log. No public item was added, removed or renamed.

## lgwks_std 4.2.0 (minor: stricter refusals)

- A capability open never blocks, and a directory listing owns its offset (#961, #913 items 4 and 5).
- A shallower path re-enters a directory a depth limit cut (#962, ADR 0048).
- A per-call user agent reaches the wire on the pooled connection (#963, ADR 0046).
- A cgroup scope is created exclusively. An existing scope is refused, never adopted (#964).
- `ps` runs from the system path with a cleared environment (#965). A child list or a `ps` start that cannot be read whole is an error, not an empty set (#967).
- `http::Options` and `http::Client` `Debug` print each header name and never its value (#970, INV-STD-HTTP-5, ADR 0050).

## lgwks_bot 8.4.0 (minor: stricter refusals)

- A record states only what the device proved (#958, #910 defects 2, 3 and 8, ADR 0044).
- `Scope::require` fails closed when a run has no authority (#959, #909 step 1, ADR 0045).
- A post that may have landed is `EffectIndeterminate`, not a failure to retry (#966).
- A step spawned inside a run keeps its run's store, definition and effect policy (#968, #909, ADR 0049).

`lgwks_ast`, `lgwks_deps`, `lgwks_macros` and `drishti` did not move.
