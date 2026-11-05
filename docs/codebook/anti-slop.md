# Codebook: anti-slop invariants

Part of [`CODEBOOK.md`](../../CODEBOOK.md), section 3.

## 3. Anti-slop invariants

Zero tolerance. Each line is a rule the gate enforces where it can and a review
rule where it cannot.

| Invariant | Required form |
|---|---|
| No production `.unwrap()` / `.expect()` | typed domain errors and `?`. Exception: compile-time constants, or an immediately preceding length check with explicit `// SAFETY:` rationale |
| No invariant-breaking `pub` | fields private by default; `pub(crate)` or accessors (`as_slice()`) |
| No defensive `.clone()` | borrows (`&str`, `&[T]`), `Cow<'a, T>`, or in-place transformation. Passing ownership is for permanent retention only |
| No mutex across `.await` | scope the lock to the critical section, or use a bounded actor channel |
| No unbounded memory or channels | `unbounded_channel()` forbidden; explicit buffer bounds; every cache declares TTL or LRU eviction |
| No fire-and-forget spawns | every task tracked in a `JoinSet` or supervisor with a `CancellationToken` |
| No strong reference cycles | back-pointers and listeners use `Weak` |
| No infinite loops without cancellation | every loop has a timeout, iteration or retry ceiling, or cancellation branch. No spin-waiting |

### 3.1 Type system as contract

- Make invalid states unrepresentable: sum types for state machines, newtypes
  for domain primitives (`UserId(u64)` not `u64`), phantom types for
  compile-time capability tracking, builders for validated construction.
- Prefer static dispatch (`impl Trait`, generics) over `Box<dyn Trait>` unless
  runtime polymorphism is the actual requirement.
- State machine as type parameter: `Connection<Disconnected>` →
  `Connection<Handshaking>` → `Connection<Ready>`. A runtime `is_ready` boolean
  is a design smell.
- `#[non_exhaustive]` on public enums and structs that will grow, with a
  `pub fn new(...)` constructor. Struct-expression construction of a
  non-exhaustive type outside its crate is `E0639`; build through the
  constructor.

### 3.2 Errors

- Typed, exhaustive error enums with structured context and `source()` chains.
  No `String` errors in library code. No `anyhow` in a public API.
- `?` propagation with explicit conversion at module boundaries.
- `type Error = Infallible` is a claim that the operation cannot fail. Do not
  use it on a port whose implementation can refuse; introduce a real error type
  so the refusal is expressible.
- `unreachable!()` is a panic. Where a value can be refused, return the typed
  error instead.

### 3.3 Arithmetic and casts

- `checked_*` / `saturating_*` for arithmetic that can overflow.
- `div_ceil` or `checked_div` for integer division.
- `try_from(...).unwrap_or(...)` instead of `as` casts. `as_conversions` is
  forbidden.

### 3.4 Reference patterns

`pattern_type_mismatch` is forbidden. Matching on `&T` or `&mut T` needs `&_`
or `&mut _` patterns, or `match *t`. Non-Copy payloads use `ref` / `ref mut`.
Slice destructuring of owned data:
```rust
let &[ref path, ref account, ref space] = arguments.as_slice() else {
    return Err(Error::WrongArity(arguments.len()));
};
```

### 3.5 Non-async `fn -> impl Future`

A plain `fn` returning `impl Future` cannot use `?` in its body. Build the
result first and wrap with `ready(result)`.

### 3.6 Locks

`Mutex::lock()` recovers from poisoning with
`.unwrap_or_else(std::sync::PoisonError::into_inner)`. Do not `unwrap()` a
poisoned lock.
