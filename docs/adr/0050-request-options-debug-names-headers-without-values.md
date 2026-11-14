# ADR 0050: request options Debug names headers without values

Status: **accepted**, 2026-11-13, for #913.

Stakeholders: every caller that logs `lgwks_std::http::Options` or `Client`; every operator who
reads those logs.
Concerns: a credential in a header value written to a log line.
Requirements: INV-STD-HTTP-5
Viewpoint: the log-safety viewpoint.
Elements and relations: `http::Options` (holds the headers), `http::Client` (holds an `Options`).

## Context

`Options` derived `Debug` over its header pairs, and `Client` derives `Debug` over its `Options`.
So `debug!(?options)` wrote an `Authorization`, `Cookie` or API key value into the log
(CWE-532).

## Decision

- `Options` has a hand-written `Debug`. It renders every field, but each extra header by name
  alone.
- `Client` keeps its derived `Debug` and inherits the redaction through its options.

## Alternatives rejected

- **A per-header sensitive flag**, as `http::HeaderValue::set_sensitive` does. Every caller must
  remember to flag, and an unflagged custom token header leaks.
- **A deny list** of `Authorization`, `Cookie` and `Proxy-Authorization`. A credential under any
  other name leaks.

## Consequences

- A debug line shows which headers were set, never their values.
- A field added to `Options` must be listed in its `Debug`, or clippy's
  `missing_fields_in_debug` refuses it.

## Correspondence

Answers #913's misuse list. No module is added or removed, so CODEBOOK.md is unchanged.

## Verification

- `tests/it/sim_http.rs` `debug_names_every_header_and_renders_no_value` draws names, casing and
  secrets per seed and checks the compact and pretty forms of both types.
