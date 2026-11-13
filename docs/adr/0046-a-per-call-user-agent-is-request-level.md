# ADR 0046: a per-call user agent is request level

Status: **accepted**, 2026-11-13, for #913 item 6.

Stakeholders: every caller of `Client::get_with` and `Client::post_with` that sets its own
`Options::user_agent`, such as a tenant-scoped fetch.
Concerns: a per-call value that the pooled path drops without a word.
Requirements: R9 (`docs/requirements/d-correctness.md`), INV-STD-HTTP-4
Viewpoint: the HTTP exchange viewpoint, one agent per client.
Elements and relations: `http::Client` (the pooled agent), `http::exchange::agent_settings_differ`
(when a call needs its own agent), `http::walk::send_hop` (what each hop sends).

## Context

The pooled agent is built with the client's user agent. `agent_settings_differ` compared only the
timeout, the TLS identity and the pool. A call with its own user agent ran on the pooled agent and
sent the client's agent instead.

## Decision

- Each hop sends the call's user agent as a `User-Agent` request header. ureq adds its configured
  value only when the request carries none, so the pooled connection is kept.
- An empty user agent goes as ureq's request-level `user_agent("")`. No header can say "none".
- A caller's explicit `User-Agent` header on that hop wins. The hop adds no second one.
- `agent_settings_differ` stays as it is: the user agent never decides pooling.

## Alternatives rejected

- **Compare the user agent in `agent_settings_differ`.** It is correct, but each such call builds a
  one-shot agent and opens its own socket. A per-tenant agent would then never pool.
- **Override ureq's request-level `user_agent` for every value.** ureq 3.5.1 sends any such
  override on a fresh connection that never enters the pool (its `connection_overrides_bypass_pool`
  test). Measured here: six calls cost four sockets.
- **Add it to the caller headers.** A cross-origin redirect drops caller headers. The user agent
  is not a credential, and every hop must carry it.

## Consequences

- A per-call user agent reaches the wire on every hop, on the pooled connection.
- An empty user agent sends no `User-Agent` header, on a connection of its own.

## Correspondence

Answers #913 item 6. No module is added or removed, so CODEBOOK.md is unchanged.

## Verification

- `http::tests::pool::a_per_call_user_agent_reaches_the_wire_on_the_pooled_connection` sends five
  per-call agents and one explicit header over one client. It asserts each value on the wire and
  two accepted connections: the pooled one, and the empty agent's own.
