# lgwks_bot 9.0.0

Released 2026-11-15. Each layer raises its own typed error, and a lower layer never names an upper one (#990, ADR 0061).

## lgwks_bot 9.0.0 (major: a moved variant breaks a match)

The one break: a consumer `match` on a `BotError` variant that moved. Every field and every `Display` text is kept, and `retry_class` answers the same on every layer.

| Was `BotError::` | Now |
|---|---|
| `CapabilityDenied`, `CredentialExpired`, `Conflict`, `DomainError`, `EffectIndeterminate`, `ActionNotDeclared`, `EffectUnsettled`, `EffectRefused` | `KernelError::` (also `lgwks_bot::error::KernelError`) |
| `IncompleteSpec`, `UnregisteredDomain`, `DuplicateDomain`, `SpecTooLarge`, `MalformedSpec`, `UnsupportedSpecVersion`, `UnknownCondition`, `TickInsideRuntime`, `PollStalled`, `UnstableObservation`, `PollDeadlineUnbounded`, `PollDeadlineExceeded`, `CredentialRejected`, `EvaluateError`, `TypeMismatch`, `PendingTransition`, `NoSuchWork`, `EvidenceSuperseded`, `EvidenceStaleAttempt`, `EvidenceContradicted`, `EffectUnrecorded` | `EcsError::` |
| the session variants (a flow refused at load, a session step refused) | `SessionError::` |

- The verbs (`Observe`, `Evaluate`, `Execute`, `Query`) return `EcsError`. A kernel failure rides inside it as `EcsError::Kernel`, so `?` on a `KernelError` still works.
- `BotError` is now the app layer's sum: `Kernel`, `World`, `Session` and `Script` wrappers with `From` for each layer. It is `#[non_exhaustive]`.
- `FlowError::Bot` carries `Box<EcsError>`.
- `upstream_credential_rejection` moved to `lgwks_bot::spec`. `EffectEvidence` moved to `lgwks_bot::journal`. `CasWrite` moved to `lgwks_bot::domain::cas`.
- `INV-ERROR-LAYER-1` holds the layering: `core_boundary` refuses a kernel module that names the world, the session or the stability monitor.

`lgwks_std`, `lgwks_ast`, `lgwks_deps`, `lgwks_macros` and `drishti` did not move.
