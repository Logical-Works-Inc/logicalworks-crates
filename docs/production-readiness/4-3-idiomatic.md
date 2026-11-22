# 4.3 Idiomatic: ownership, errors, lifetimes sound

Part of [production readiness](../production-readiness.md), §4, nine axes.

**✅ Compiler-enforced, with one named hole in the proof chain.**

- `unsafe_code = "forbid"` at workspace scope, with `lgwks_std`'s two syscall
  modules as the named exception (INV-STD-UNSAFE-1). Not `deny`. A `deny` is defeated
  by any single `#[allow]` anywhere, including one smuggled in by a macro this
  workspace does not own. A `forbid` cannot be lowered from source, and a
  conflicting allow is a hard `E0453`.
- The lint table in the root `Cargo.toml`. Its counts, the four corpus names that
  silently did nothing, and what no lint can catch (Miri, perf counters, allocation
  profiling, review) are [`lints.md`](../codebook/lints.md) §2.2 and §2.3.
- Every `#[allow]` and `#[expect]` must carry `reason = "..."`.
- `clippy --workspace --all-targets --locked -- -D warnings` is a required gate
  lane and is green, across a feature matrix and a `--no-default-features` pass.
- Integration test files covering authority, tick semantics, the effect
  journal, durable dispatch, identity, flow parsing, locator eligibility,
  process ownership, the async tier, and the no-default build.

**The named hole.** [`proofs/`](../../proofs/README.md) kernel-checks seven theorems
about the condition language and the capability gate. It is **not a refinement
proof**. The theorems are about a hand-written abstract model, and nothing proves
that `spec.rs` implements that model. The accurate form of the claim is "the design
has these properties, and the implementation is argued not to express the states
that would break them." That second half is an argument.
