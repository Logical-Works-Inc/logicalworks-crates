# ADR 0067: a capability scope refuses a traversal segment

Status: **accepted**, 2026-11-19. Corrects #779.

Stakeholders: the Director; a consumer that maps a token's resource onto files, keys or routes; an operator who logs.
Concerns: a token scoped to `/notes/alice` allowed `/notes/alice/../bob`; `Debug` printed the root key; a denial's
trace did not say which rule denied.
Requirements: R9 (`docs/requirements/d-correctness.md`), INV-BOT-191
Viewpoint: the authority viewpoint, one issuer, as a consumer reads a decision.
Elements and relations: `cap_token::valid_resource`, `cap_token::Authority`, `cap_token::policy::Request`,
`Policy::decide` and its `TraceStep`s.

## Context

`under_scope` compares strings on `/` boundaries, so `/notes/alice/../bob` sat under `/notes/alice`. The consumer
that runs the effect resolves the name, and resolves it to `/notes/bob`. CWE-23 to CWE-35 list the other spellings:
`\..\`, `...`, `....`, and percent escapes a consumer decodes later. A `Request` decoded from JSON skipped
`Request::new`. `Authority` derived `Debug`, which prints its 32 root bytes. `decide` pushed the audience, action and
resource steps together, so every denial carried all three, and a dummy value was folded into every inputs hash.

## Decision

1. **A resource segment that is empty, only dots, or holds `\` or `%` is refused** as `TokenError::BadText` for
   `resource`, in `Request::new` and `Caveat::new`. A dot inside a name (`alice.txt`, `.profile`) is admitted.
2. **A decoded `Request` is validated**: it deserializes through `try_from` into `Request::new`.
3. **`Authority`'s `Debug` is `Authority { root: <redacted> }`.**
4. **The walk stops at the first rule that denies.** That step is the last in the trace, and its inputs open with
   `grant=<what the token holds>`. Each denial emits the debug event `cap_token_denied` with the rule.

## Alternatives rejected

- **Normalize dot segments (RFC 3986 5.2.4) and judge the result.** One rule here does not match a consumer that
  splits on `\` or decodes `%2e`, so the verdict and the effect could disagree. Reverse this only if every consumer
  provably resolves names with the same normalizer this crate ships.
- **Refuse only `..`.** CWE-32 and CWE-33 show `...` and `....` resolve upward on some hosts.

## Consequences

- A caller with a legitimate name that is only dots or holds `%` or `\` renames it. None in the estate does.
- `inputs_hash` values change, since the dummy byte is gone. Nothing stores them across versions.

## Correspondence

Refines the token design of #779. Relates to ADR 0045 (a run carries its authority on its scope).

## Verification

`tests/it/sim_cap_token_policy.rs`: `a_dot_dot_segment_is_refused_typed`, `every_traversal_spelling_is_refused`,
`a_deserialized_request_is_validated`, `authority_debug_redacts_the_root_key`, and the 4,096-resource seeded sweep
against the segment model. `tests/it/cap_tokens.rs`: the action and audience denials end at their rule.
