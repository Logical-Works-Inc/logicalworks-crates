# ADR 0020: what the edge answers itself

Status: **accepted**, 2026-11-08. Implements #879 and #880.

Stakeholders: the Director; app authors on `lgwks_bot::app`; operators behind a load balancer.
Concerns: a probe that creates a tenant; a key in a body; an unknown caller spending a mailbox slot; a key reaching a tenant's log.
Requirements: R9, R10
Viewpoint: the request viewpoint, following one request from the socket to a cell.
Elements and relations: `rt::edge::liveness`, called by the world's ingress and the app's; `App::authenticate`, `App::route_as`, `Principal` and `RequestHeaders` in `app::auth`; `RequestCounts::probes`; the `edge-load` binary.

## Context

Every request named a tenant, so a probe either got `400` or held a cell for a name that is no tenant. A route handler saw only its decoded body, so a key had to travel in JSON, where anything that logs bodies keeps it.

## Decision

1. **The edge answers `GET` and `HEAD` `/healthz`**, on both edges, before reading a tenant: `200` while it accepts and `503` once its `EdgeStop` has fired. A probe needs no key.
2. **An authenticator runs at the edge**, after the tenant name and before the verb or the body. `None` is `401` with `WWW-Authenticate: Bearer`, as RFC 6750 asks, and nothing is read or queued.
3. **Only the `Principal` enters the cell.** The handler never sees a header, so a key cannot reach a tenant's log.
4. **The view is bounded.** A head past 64 headers or 16 KiB is `431` without asking. A header sent twice reads as absent, so a proxy and this edge cannot disagree about which counts.
5. **`App::check` refuses a named route with no authenticator**, so nothing serves with nobody to name a caller.

## Alternatives rejected

- **A header view in the handler.** It costs a mailbox slot before refusing, and puts headers in the tenant's log.
- **A tenant-scoped probe.** It holds a cell for a name that is no tenant and counts toward the tenant ceiling.
- **A refusal type with a reason.** An unknown caller gains nothing from a reason, and one status keeps the authenticator small.

## Consequences

- `/healthz` is no longer `400` on either edge, and a tenant cannot own that path at the root because tenant paths start at `/t/`.
- Only the world's edge counts probes, in `RequestCounts`. An app's host counts are not public, so an app's probes are uncounted.
- Over a socket a stopped edge refuses the connection. `503` is what a request already on a connection reads during the drain.
- A panicking authenticator drops that connection and admits nothing.
- Comparing a secret is the authenticator's work. This crate does not compare keys for it.

## Correspondence

Sits in front of the cell host of ADR 0006.

## Verification

`tests/it/sim_app_edge.rs` holds two tenants' keys to a model over seeded requests, with weird bytes, a bounded head, a panic and a hundred racing callers. `tests/it/edge_load_keyed.rs` drives the shipped binary, and `rt::edge::tests` holds the `503`.
