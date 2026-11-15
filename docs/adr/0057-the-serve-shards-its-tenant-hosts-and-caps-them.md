# ADR 0057: the serve shards its tenant hosts and caps them

Status: **accepted**, 2026-11-14. Implements the second step of #1008, from #823.

Stakeholders: the Director; consumers that serve many tenants through `rt::serve`; `lgwks_bot` maintainers.
Concerns: one tenant's first request holding every other tenant's request; a flood of tenant names growing a table, and a store file and thread each, without bound; two requests opening one tenant's advisory-locked run store; a drain that cancels in a different order each time.
Requirements: R9, R10
Viewpoint: the tenant viewpoint, following one tenant name from the path to its host and back.
Elements and relations: `rt::serve::hosts::Hosts`, `MAX_TENANT_HOSTS`, `ServeError::TooManyTenants`, `Shared::hosts`, `ingress::host_for`, `drain_owned`.

## Context

ADR 0056 left the tenant hosts in one `Mutex<HashMap>` and said it would be extended when that changed. The lock was held while a host was built, and a build opens the tenant's run store file under an advisory lock. Every request of every tenant therefore waited behind any tenant's first request, and the table had no bound: any valid tenant name added a host, a file handle and a storage thread.

## Decision

1. **The table is 64 shards.** Each is a read-write lock over an ordered map of tenant name to slot. The shard is the tenant name hashed with `DefaultHasher::new()`, whose keys are fixed, masked to 64.
2. **A slot builds its host once.** A slot holds a `OnceLock<Host>` and a build lock. A lookup of a built host takes one shard's read lock for a map get and a clone. A new tenant takes the write lock only to add an empty slot, then builds under the slot's own lock, so tenants never wait on each other's build and one tenant's store is opened by one builder. A failed build leaves the slot empty and the next request builds again.
3. **The table is capped at 1,024 tenants.** `MAX_TENANT_HOSTS` is claimed with one atomic step before a slot exists, so concurrent first requests never pass it. A new tenant past it is refused as `ServeError::TooManyTenants { cap }` and answered `503` with `Retry-After`; a known tenant is served as before. Each built slot is a store file and a storage thread, so the cap is the file and thread budget.
4. **The drain walks the hosts in tenant order.** `Hosts::all` collects the built hosts across the shards and sorts them by tenant, so a drain cancels the same way every time.
5. **No lock is held across an await.** The locks are `std` locks over short synchronous work, and a poisoned lock is recovered with a logged event, as `Runs` does.

## Alternatives rejected

- **A concurrent map crate (`dashmap`).** It is a new dependency for a table the standard library covers, and its entry API holds a shard lock across the host build.
- **One `RwLock<HashMap>`.** Reads share it, but the first request of any tenant still takes the write lock for the whole build.
- **No cap.** It keeps today's behaviour and today's flood: a name per request.
- **A cap that evicts the least recent tenant.** An evicted host has an open store and runs in flight; closing it would break the run's log-first guarantee.

## Consequences

- A tenant past the cap waits for no one and is told to retry; nothing frees a slot until the serve stops, so the cap is a ceiling per serve, not a rate.
- A slot whose build failed still counts against the cap; the cap bounds slots, not built hosts.
- The shard walk of `Hosts::all` takes each shard's read lock in turn, so a host added during a drain may or may not be cancelled; the serve has stopped admitting by then.
- Measured on the 2-CPU 2 GB Linux box, the rig rows of this change are in the commit message.

## Correspondence

Refines ADR 0056, whose last consequence this answers, and relates to ADR 0053, which gives each tenant its own world.

## Verification

- `crates/lgwks-bot/src/rt/serve/hosts.rs`: `sim_each_tenant_is_built_once_under_threads` (seeded, eight threads), `a_parked_build_for_one_tenant_does_not_hold_another`, `past_the_cap_a_new_tenant_is_refused_and_a_known_one_is_served`, `a_failed_build_is_retried_and_all_lists_hosts_in_tenant_order` and `a_tenant_hashes_to_one_stable_shard_and_names_spread`.
- `crates/lgwks-bot/tests/it/sim_serve_hop.rs` and `sim_serve.rs` hold the answers unchanged across tenants.
- INV-SERVE-2 in `crates/lgwks-bot/INVARIANTS-3.md`.
