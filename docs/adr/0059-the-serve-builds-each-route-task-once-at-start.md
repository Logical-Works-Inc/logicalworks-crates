# ADR 0059: the serve builds each route's task once, at start

Status: **accepted**, 2026-11-14. Implements the third step of #1008, from #823.

Stakeholders: the Director; consumers that serve routes through `rt::serve`; `lgwks_bot` maintainers.
Concerns: work a request repeats that a route could do once; a request's tenant and content type leaking into the identity of its key; a route whose verb the edge cannot dispatch surfacing only as a 404 later.
Requirements: R9, R10
Viewpoint: the request viewpoint, following one request from its path to its route's task and the run it starts.
Elements and relations: `rt::serve::table::Table`, `RouteTask`, `ServeInput`, `Shared::routes`, `ingress::Admitted`, `serve_async`.

## Context

ADR 0056 made each request a run the serve owns. A request still found its route by a scan of the route list, then built a task for it: it validated the name `serve-route`, made a closure, and cloned the handler, tenant and content type into that closure. None of that depends on the request. The route list is also fixed once the serve starts.

## Decision

1. **The routes become a table at start.** `Table::build` runs in `serve_async` before the listener converts. It holds one map of path to task for `GET` and one for `POST`, so a request finds its task with one ordered-map lookup by path, and the lookup clones an `Arc`.
2. **One task serves every tenant.** The task's body is a boxed `Send + Sync` closure built from the route's handler. The tenant and the content type travel in `ServeInput` beside the body, so the task captures nothing of the request.
3. **Only the body is the request's identity.** `write_identity` frames the body alone, as it did when the content type was captured in the closure. A repeat of a key and body from another content type reattaches to the first answer, as before.
4. **A second route on a verb and path is dropped.** The first one wins, as the first match won in the scan.
5. **A verb the edge cannot dispatch is refused at start.** `RouteMethod` is non-exhaustive; a verb other than `GET` and `POST` is a typed `ServeError::Host`, logged, instead of a route no request can reach.
6. **The body stays a `Vec<u8>`.** `TypedRoute::new` is public with that shape, so a borrowed body would change the public surface.

## Alternatives rejected

- **A `HashMap` keyed by verb and path.** It hashes the path on every request; an ordered map of a handful of routes needs no hash and keeps the table in a stable order.
- **Building the task per tenant.** It saves nothing a shared task does not, and it makes the table grow with the tenant count that ADR 0057 caps.
- **Putting the tenant and content type into the identity.** It would turn the same key and body from a second content type into a conflict, which the edge never did.

## Consequences

- The table is fixed at start: a route added later needs a new serve.
- A route whose verb is not served now stops the start instead of answering 404 for ever.
- The body is still copied from the connection once per request, as the public handler shape needs an owned vector.
- Measured on the 2-CPU 2 GB Linux box, the rig rows of this change are in the commit message.

## Correspondence

Refines ADR 0056, whose request path this shortens, and relates to ADR 0057, which keeps the tenant table this one does not grow.

## Verification

- `crates/lgwks-bot/src/rt/serve/table.rs`: `a_request_finds_its_route_by_verb_and_path`, `sim_one_route_task_serves_every_tenant_its_own_input` (seeded, real hosts) and `sim_route_table_bands_replay_from_their_seed`.
- `crates/lgwks-bot/src/rt/serve/ingress.rs`: `only_the_body_is_the_identity_of_a_serve_input`.
- `crates/lgwks-bot/tests/it/sim_serve_hop.rs`, `sim_serve.rs` and `serve.rs` hold the answers unchanged.
- INV-SERVE-3 in `crates/lgwks-bot/INVARIANTS-3.md`.
