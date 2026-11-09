# Bot semantics on an ECS substrate: The GPUI assessment

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 6. The gpui assessment

**gpui is not an acceleration layer for non-UI work.** This is not a judgement
call; it follows from what the crate exposes.

- `gpui` 0.2.2 is on crates.io and is **already admitted**: `contract/APPROVED.toml`
  records `crate = "gpui"`, `tier = "boundary"`, `owner = "lgwks_deps"`,
  `capability = "ui.gpu-desktop"`, wired as the storefront feature `gpui`.
- But the capability name is accurate. Layout is **taffy** (CPU, pinned `=0.9.0`
  by gpui 0.2.2: `SECURITY.md` records the `grid` advisory reached through it,
  and why it is unreachable),
  text shaping is **cosmic-text** (CPU), element diffing is CPU, and the GPU
  surface is a `Scene` of paint primitives submitted through `blade-graphics`.
  **There is no exposed compute, buffer, or shader-dispatch API.**
- `default = ["font-kit", "wayland", "x11", "windows-manifest"]`, and even
  `test-support` enables wayland+x11. Adopting it headless drags in
  wayland/x11/blade/objc2/font-kit for zero speedup.

So: gpui is the right dependency for *a bot console* (a real dashboard over a
running bot) and the wrong one for making the bot faster. It is a UI toolkit,
not a compute layer.

**The reusable part is the scheduler, not the GPU.** GPUI's `TestDispatcher` is
the strongest determinism design available in the ecosystem:
`TestSchedulerConfig { seed,
randomize_order: true, allow_parking: false }`, a virtual clock
(`advance_clock`, `advance_clock_to_next_timer`, `run_until_parked`), and seeded
interleaving so seeds *sweep* real schedules and replay failures exactly. That is
strictly stronger than "single-threaded".

**And it is not reachable as a dependency.** The `scheduler` crate is not
published on crates.io (the crate of that name there is an unrelated 2016 Linux
affinity binding), so zed's is available only as a path or git dependency on the
monorepo.

That leaves the scheduler decision in §9.

## 7. The control plane: bors and Zuul

bors is worth studying because it is the canonical **dumb obedient bot**: a tiny
legible state machine, a narrow command grammar, and no cleverness. Its lessons
transfer directly, and they are all about durability rather than concurrency.

**Adopted as invariants:**

1. **Derive queue membership; never store it.** bors's `Patch` has no "queued"
   flag: "queued" is a *query* (`open AND no active-batch link`). A stored enum
   is a second source of truth that drifts on crash. In `lgwks_bot` terms: a
   chain's readiness is a predicate over observed facts, not a field.
2. **One idempotent reconciler; events only wake it.** The webhook and the timer
   call the same guarded transition. Recompute ordering from durable rows each
   tick; never hold the queue in memory.
3. **Checkpoint after the side effect, in a single write.** bors pushes to
   staging and *then* writes `{commit, state: running}` in one UPDATE. A crash
   before the write leaves the item "not started", which is the safe default.
4. **The durable log is the external system.** bors re-reads CI status from
   GitHub rather than trusting local rows. Local state caches *intent*, never
   *truth*.
5. **Retry is replay of recorded intent.** `bors retry` re-runs the stored
   `(actor, command)` pair: it re-executes a recorded authorization rather than
   re-deriving one. This is the property the journal holds, arrived at from a
   different direction.
6. **Authorize at the parse boundary, over the whole command list**: parse
   first, reduce to one required level, authorize once, all-or-nothing. bors
   reduces `[:try, {:activate_by, ..}]` to a single `:reviewer` level before any
   effect. This is `Auth`/`GrantSet`, and the ordering (parse → reduce →
   authorize → act) is the part worth copying.
7. **Unknown input parses to nothing; it is not an error.** bors's fallthrough
   returns `[]`. A bot that errors on unrecognized text is a bot that can be
   wedged by a stranger's comment.

**And one structural correction:** bors is a *degenerate case of Zuul*. Zuul adds
declared pipelines with named managers (`independent` / `dependent` / `serial`),
**windows as speculative rate limiting** with TCP-style auto-tuning, enumerated
reporting outcomes, and cross-project `Depends-On`. If this workspace ever grows
past one queue, Zuul's pipeline model is the reference, not bors's single queue.

**Not copied: batching.** bors amortizes CI by testing several changes at once
and pays for it with bisection on failure. A bot whose tick is cheap does not
need that trade, and every batch is a place where a failure cannot be attributed.
