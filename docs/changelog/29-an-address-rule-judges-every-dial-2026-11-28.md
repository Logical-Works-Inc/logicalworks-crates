# lgwks_std 5.1.0

Cut 2026-11-28. `lgwks_std::http` takes an address rule, so a service can dial a URL a tenant supplied (#1170). New public items, so this is a minor.

## What a consumer notices

- `Options::address_rule(|ip| ...)` judges every address the exchange could dial, on every hop, before a socket opens. The system resolver hands over its first 16 answers.
- A refused address is `Error::AddressRefused { host, address }`. `Error` is non-exhaustive, so a match already has a wildcard.
- A call with a rule or a lookup uses no proxy. A call with neither is as before.
- A `Client` built with a rule keeps it for a call whose options carry none.

## lgwks_std 5.1.0 (minor)

- `Options::address_rule` and `Options::lookup` (ADR 0094, INV-STD-HTTP-6).
- An IPv4-mapped IPv6 answer reaches the rule as its IPv4 address.
- The `ureq` requirement is now `3.5.2` or later in the 3 series.
- The `lgwks_bot` `estate` smoke runs the rule on the shipped binary. No library behaviour of `lgwks_bot` changes.
