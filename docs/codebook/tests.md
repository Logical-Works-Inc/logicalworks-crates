# Codebook: tests, comments and suppressions

Part of [`CODEBOOK.md`](../../CODEBOOK.md), sections 7 to 9.

## 7. Tests

- **Unit**: pure functions, isolated logic, fast, deterministic, no I/O.
- **Integration**: real module boundaries with real or containerized
  dependencies. Test the contract: these inputs produce these outputs and this
  persisted state.
- **End-to-end journeys**: critical user paths through public APIs from clean
  state. Happy path, error paths, concurrent access, crash-and-recover. At least
  10% of e2e tests induce real process kills or panic injection and verify
  recovery and data integrity.
- **Property-based**: for algorithms and parsers, explore the input space and
  encode round-trip and permutation invariants.

Every assertion checks a real value:
```rust
// hollow
assert!(result.is_ok());
assert!(db.count() > 0);

// real
assert_eq!(response.status, StatusCode::OK, "unauthenticated read must be refused");
assert_eq!(db.count(), 5, "five committed documents must survive restart");
```

Test style for this estate:

- `fn test() -> Result<(), Box<dyn Error>>` and `?` inside. Fixture helpers
  bubble `Result`.
- Every assert carries a trailing message.
- Every private item carries a `///` doc.
- No `unwrap` / `expect` / `panic` / `unreachable!` in test code either. The
  `clippy.toml` has no `allow-*-in-tests` relaxation on purpose.

A happy-path-only suite is a failing implementation. Mocks may assist but cannot
be the only proof.

---

## 8. Comments and naming

Comments state constraints the code cannot show. No narration, no history, no
reassurance. If a comment says what the next line does, delete the comment.

Name for systems design and architectural fidelity. A name states the entity's
role in the architecture using global, industry-standard terms, not internal
shorthand or implementation incidentals. An outside architect should recognize
it: "content-addressed fact log", "materialized read-model by replay",
"bounded capability vocabulary". A name that misleads is bad context; kill it at
the source.

Match the file you are in: naming, comment density, idiom.

---

## 9. Suppressions

Every `#[allow(...)]` and `#[expect(...)]` carries `reason = "..."`:
```rust
#[expect(unsafe_code, reason = "FoundationDB network-boot FFI leaf; no safe binding exists")]
```

`expect` is a suppression exactly as `allow` is. A reasonless suppression is
invisible to the detector and is a rule that does not exist. `allow_attributes`
and `allow_attributes_without_reason` are `forbid` so the compiler enforces this.

Never widen a lint level, edit a pinned version, or add a consumer to a register
to clear a refusal. That is a suppressed gate, not a fix.
