# ADR 0034: a stream, a key and a tree stop read whole

Status: **accepted**, 2026-11-12. Issue #913 named the three `lgwks_std` defects this record
answers, from the architecture review of 2026-11-10.

Stakeholders: an agent reading a model's event stream; a client presenting an RSA-4096 identity;
a supervisor stopping a process tree.
Concerns: an answer that depends on how the network split the bytes, a standard key the client
refuses, and a stop that leaves part of a tree running.
Requirements: R12
Viewpoint: the module viewpoint, one element per `lgwks_std` module and the reader it owns.
Elements and relations: `http::stream`, which feeds each read to the I/O-free
`http::stream::sse_decode`; `http::tls`, which reads certificates and keys through the private
`http::tls::der`; and `process`, whose Windows tree kill and Unix orphan reap share
`signal_every`.

## Context

A `\r\n` split across two reads ended two lines, so an event could dispatch early. Each read
rescanned the pending line from its start, so a long line cost time quadratic in its length. An
RSA-4096 modulus is 513 DER content bytes and the bound was 512, so every such key was refused.
The Windows tree kill and the Unix orphan reap returned at their first refusal. One access-denied
descendant then left every later target running.

## Decision

Frame events in a decoder with no I/O that holds a trailing CR across reads and scans each byte
once. Read the RSA public key of a certificate and of a key through one reader, bounded at 8,192
bits of magnitude. Stop a tree by trying every target and reporting the first refusal after the
last; on Windows, stop the root first.

## Alternatives rejected

- **Normalize line endings per read.** A per-read replace cannot pair a CR at the end of one read
  with the LF opening the next, so a split CRLF still counts twice.
- **Buffer the whole body before parsing.** Framing is trivially right, but memory is unbounded and
  a long-lived stream never yields an event.
- **Raise the content-length bound to 513 bytes.** It admits 4,096 bits and still refuses 8,192, and
  a magnitude without its sign byte would pass on length alone.

## Consequences

- `tls.rs` was past the 750-line size law; the DER walk moved to `tls/der.rs` unchanged apart from
  the shared reader.
- A stop now reports the first refusal only after every target was tried, so a caller sees the same
  error later in the sweep than before.
- Measured: `cargo nextest run --locked -p lgwks_std --all-features` ran 1,373 tests, 1,372 passed,
  in 322.6 s wall at 899 MB peak resident set. The one timeout was
  `a_self_stopping_shell_survives_until_continued` at load average 21; run alone it passed in 0.030 s.

## Correspondence

Relates to ADR 0015, whose ring TLS provider reads the keys this reader admits.

## Verification

- `http::stream::sse_decode::sim_sse` checks every split of every seeded stream against a model
  written from WHATWG HTML (INV-STD-HTTP-3).
- `http::tls::der::sim_rsa_bounds` admits every width up to 8,192 bits and refuses one byte more
  (INV-TLS-2).
- `process::sim_signal_every` signals 1,024 seeded trees whole (INV-STD-PROCESS-2).
