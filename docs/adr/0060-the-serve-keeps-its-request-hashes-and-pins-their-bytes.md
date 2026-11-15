# ADR 0060: the serve keeps its request hashes and pins their bytes

Status: **accepted**, 2026-11-14. Closes the fourth step of #1008, from #823.

Stakeholders: the Director; operators holding durable run stores; consumers that serve through `rt::serve`; `lgwks_bot` maintainers.
Concerns: a request hashing work its profile shows; a faster hash that orphans every stored receipt and run; a cost claimed without a number.
Requirements: R9, R10
Viewpoint: the request viewpoint, following one keyed request through the two hashes it pays before its body runs.
Elements and relations: `task::request::InputDigest::of`, `task::request::derive_run`, `Host::submit`, `examples/serve_digest.rs`.

## Context

The profile of the serve edge showed blake3 frames under `InputDigest::of` and `derive_run`, both inside `Host::submit`. The first is the receipt's input digest, which tells a repeat from a conflict. The second derives the run a request key names, which is how a receipt is found again after a restart. The rig always sends an `Idempotency-Key`, so the keyless body hash in `ingress` never runs on a rig row.

## Decision

1. **Both hashes stay as they are.** The measured cost is under the noise of the rig, and any change to them changes stored bytes.
2. **The cost is measured, not argued.** `examples/serve_digest.rs` times both on the shapes the rig sends and on larger bodies, and prints p50, p95 and p99 per call. The box is the 2-CPU 2 GB Linux box, a release build pinned to one core, with 2,048 samples of 256 calls. The p50, p95 and p99 of `derive_run` are 300, 311 and 363 ns. The p50 of `InputDigest::of` is 169 ns for an empty body, 267 ns for 64 bytes, 1,855 ns for 1 KiB and 17,570 ns for 16 KiB.
3. **The share is 0.28% of a request.** A rig request costs the serve 169 µs of CPU (5,530 ms over a round of 32,768 requests, serve at 64 connections), and the two hashes cost 469 ns of it. Removing both would not show at a 10% noise floor.
4. **The bytes are pinned.** Two unit tests in `task/request.rs` hold the run a tenant and key derive and the digest of an empty body to fixed hex, and hold that the run follows the tenant and key and the digest follows the body. A hash change fails them before it can orphan a receipt.

## Alternatives rejected

- **A cheaper hash for the run or the digest.** It saves under 0.3% of a request and makes every stored receipt unfindable: the derived run and the stored digest are the durable names.
- **Hashing only the bytes the receipt needs.** The digest already frames the schema id and the body and nothing else, and the run frames the domain, tenant and key; there is no byte to leave out.
- **Reusing one digest per request.** A keyed request computes each hash once. A keyless request hashes its body for the key and again for the digest, but changing the key derivation would make a keyless retry across an upgrade run again, and the rig does not send keyless requests.

## Consequences

- A body costs about 1.8 ns a byte to digest, so a large body pays: 16 KiB is 17.6 µs, and a keyless request pays it twice. That is the one place a hash change would show, and it needs its own migration of stored keys and receipts.
- The other keys a request derives (`definition_for`, the two reserved step keys, the in-run step keys) are each of the same order, a few hundred nanoseconds, and each is a stored name for the same reason.
- The example mirrors `derive_run`'s bytes, since the function is crate-private; the pin test holds the mirror honest against the real function's output.

## Correspondence

Closes the series of ADR 0056, 0057 and 0059 on the serve's request path, and relates to ADR 0049, which carries the run's store and definition on its scope.

## Verification

- `crates/lgwks-bot/examples/serve_digest.rs` prints the p50, p95 and p99 above.
- `crates/lgwks-bot/src/task/request.rs`: `a_request_run_and_input_digest_are_pinned_bytes` and `the_run_follows_the_tenant_and_key_and_the_digest_the_body`.
- INV-SERVE-4 in `crates/lgwks-bot/INVARIANTS-3.md`.
