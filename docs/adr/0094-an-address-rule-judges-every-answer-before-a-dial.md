# ADR 0094: an address rule judges every answer before a dial

Status: **accepted**, 2026-11-28.

Stakeholders: the Director; every service that dials a URL a tenant supplied.
Concerns: a supplied URL must not reach a private, loopback or link-local address (CWE-918), DNS rebinding included.
Requirements: INV-STD-HTTP-6.
Viewpoint: the trust-boundary view: where a host name becomes a socket address.
Elements and relations: `http::address` and its resolver; `Options::address_rule`, `Options::lookup`,
`Error::AddressRefused`; `agent_for` and `Client::exchange`.

## Context

`lgwks_std::http` built its ureq agent with no seam, and no other repository may use `ureq` directly (#1170). A check
that resolves a name before the call is a second lookup, so a rebinding name passes it. ureq 3.5.2 resolves once for
each request, before it takes a pooled connection or opens one. A new connection dials only what that call returned
(`run.rs` `connect`).

## Decision

1. `Options::address_rule` takes `Fn(IpAddr) -> bool`. An agent with a rule resolves through our resolver, which judges
   every answer ureq would dial. One refused answer refuses the hop with `Error::AddressRefused`.
2. An IPv4-mapped IPv6 answer is judged as its IPv4 address, which is what its socket reaches.
3. A rule or a lookup turns the proxy off, the environment's included.
4. `Options::lookup` answers names in place of the system resolver, so a test names answers with no DNS.
5. The rule and lookup are agent settings: a call that changes one leaves the pool. A `Client` call that carries none
   keeps the client's.
6. The `ureq` floor is 3.5.2, and the lockfile holds it (ADR 0031).

## Alternatives rejected

- **Resolve and check, then let ureq resolve again.** A rebinding name answers the two lookups differently.
- **Rewrite the URL to the checked address.** TLS then verifies the certificate against the address, so HTTPS breaks.
- **Dial the admitted answers and drop the rest.** A name that answers inward would keep a way through.
- **A built-in "public only" rule.** Which ranges are inward depends on the deployment. The caller owns that list.
- **Pin `ureq` to one minor.** ADR 0031 makes a requirement a caret.

## Consequences

- The resolver seam is ureq's `unversioned` module. A later ureq minor can break this build for a consumer who updates
  past the lock. Reverse the caret if one release does.
- The call's timeouts cannot interrupt a caller's lookup.
- The system resolver path keeps ureq's first 16 answers, so the rest are neither judged nor dialled.
- A pooled connection is reused only after the new answer passes the rule. It may reach an earlier admitted address.
- No new dependency. With neither setting, the agent is built as before.

## Correspondence

Refines ADR 0015 by keeping one HTTP owner. Follows ADR 0031.

## Verification

`crates/lgwks-std/tests/it/sim_http_address.rs`: a metadata answer, a rebinding redirect, 2,000 seeded mixed answers,
2,000 seeded IPv4-mapped answers, IP literals, the client's kept rule, the pool, and a child process under `HTTP_PROXY`.

The `estate` binary's smoke (`vocabulary/inward.rs`) refuses a loopback URL and a metadata answer and completes an
admitted one, on the shipped binary; `the_smoke_binary_refuses_a_url_that_reaches_inward` holds it. No library code in
this workspace sets a rule. The product consumer is larry#510 part 2.
