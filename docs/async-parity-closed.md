# Async runtime parity: Closed capabilities

Part of [Async runtime parity](async-parity.md).

## 3. The capabilities that were closed, and why each mattered

### Spawning at all: the crate's own thesis decided it, twice

Every verb in `lgwks_bot` is deliberately **not** `Send`, so a domain may hold
thread-local state. That is why `BoxFuture` is unconstrained and why the crate
carries its one `#[allow(async_fn_in_trait)]`. `rt::task::spawn` required
`F: Future + Send`, so a future with the property the crate is built around had
nowhere to go; 0.4.0 answered that by exporting `LocalSet` and `spawn_local`.

That answer is withdrawn, and the reason generalises past it: a single-threaded
spawn returns the *same droppable handle* as the multi-threaded one. A caller
could start a local task and forget it exactly as easily, with the added trap
that the handle's type could not be named at the call site, so it could not even
be stored. The thesis — no verb here starts work and hands back a handle to it —
decides both.

What serves the thesis is *driving* instead of spawning: await the future
directly. `tests/it/rt_async_tier.rs::a_non_send_future_is_driven_rather_than_spawned`
polls a genuinely `!Send` future (`Rc<Cell<u8>>`) to completion on the calling
thread. Asserting the bound exists would not have proved anything; the test
compiles only because the future is not `Send`.

**The two escape routes this page used to recommend do not exist, and naming
them was a false claim about the API.** Both
[`rt::task::join_all_bounded`](https://docs.rs/lgwks_bot) and
`Supervisor::spawn` are declared `F: Future + Send + 'static` in this revision:
`join_all_bounded` internally spawns onto a `JoinSet`, and a `Supervisor` owns
its work as `Send` tasks. Neither accepts a borrowed or `!Send` future, so "run
it through `join_all_bounded`, or place it on a `Supervisor`" was advice that
does not compile for exactly the futures a `!Send` domain holds.

What remains is: **await it on the task that owns it.** A borrowed or
non-`Send` future is driven inline. The supervised form of that is
`lgwks_bot::task::Host::run` (#87): it polls a task's body on the calling task, so the
body need not be `Send` and its input may borrow. Its report carries the
disposition and a bounded step trail. Bounded fan-out over such futures is still a
caller's own single-threaded driver, because the ceiling, supervision and reporting
of `join_all_bounded` and `Supervisor` are for `Send` work and do not transfer.

### `CancellationToken`: the supervision rule named a type that did not exist

The supervision rule requires that every background task be tracked in a
`JoinSet` or supervisor and listen to a `CancellationToken`. The `JoinSet` half
shipped; the token was absent from the workspace entirely. A rule that names a
type the caller cannot obtain is enforceable against the wrong practice and
unenforceable in favour of the right one.

It is written in-crate on `tokio::sync::watch` rather than admitted from
`tokio-util`, because the storefront edge would exist to carry one type. The `watch` channel
was chosen over `Notify` deliberately: `notify_waiters` wakes only the waiters
registered *at that instant*, so a `cancelled()` future created a microsecond
after `cancel()` would hang forever. A `watch` carries the state as a value, so
there is no window.

The parent link points **up**. The first implementation linked down with `Weak`
children, and `a_descendant_survives_its_intermediate_parent_being_dropped`
caught it: dropping a mid-chain token orphaned its descendants, so a leaf held
by a spawned task silently became uncancellable, the one failure a cancellation
primitive must not have.

### `rt::io`: the drivers could not be read from

Absent entirely: no `AsyncRead`/`AsyncWrite`, no `BufReader`, no `copy`. A
consumer could open a socket or a pipe and still not read from it without naming
`tokio`, which is the thing the facade exists to prevent. This one needed a
storefront change, because `io-util` was reachable only by enabling the entire
networking stack: `tokio-io` is now its own rung and `tokio-net` builds on it.

## 4. The two that remain open

These are stated rather than left implied, because a capability assumed to
exist costs a search that cannot succeed.

1. **`block_in_place`.** For CPU-bound work inside an async task on a
   multi-thread runtime. `lgwks_std::task::spawn_blocking` covers the common
   case; this covers the case where the future cannot be `'static`.
2. **`shutdown_background`.** `shutdown_timeout` exists; the non-waiting form
   does not.
