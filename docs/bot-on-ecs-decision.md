# Bot semantics on an ECS substrate: The open decision

Part of [Bot semantics on an ECS substrate](bot-on-ecs.md).

## 9. The open decision

The ECS admission is **made**: `bevy_ecs`, `bevy_app`, `bevy_time`, and
`bevy_state` are registered in `contract/APPROVED.toml` and `lgwks-deps check .`
accepts them (`docs/bevy-admission.md`). Measured in this working tree rather
than estimated: **60 packages** for `bevy-ecs`, 63 for `bevy-app`, 64 for
`bevy-time`, 62 for `bevy-state`, with `bevy_reflect` **absent** from the
compiled tree. Pinned `^0.19`; 0.20 is an RC and is not taken.

**The scheduler is decided, against the reuse instruction's preference.** The
project owner took the `lgwks_std` module on 2026-09-21, having been shown both
rungs and the finding below. The two options were:

- **VENDOR (ladder rung 7).** Take zed's `scheduler` crate as audited source into
  `vendor/` and extend it for the clock and RNG needs of this workspace. The `vendor/` tree
  existed then (294 crates, since removed under INV-DEP-17), and `lgwks-deps vendor`
  is the rung-7 tool. This
  gets the seeded-interleaving scheduler without taking gpui's UI closure.
- **ELIMINATE, against the instruction.** Write the seeded scheduler as an
  `lgwks_std` module. Cheaper at the boundary, but it is rebuilding something
  that already exists and works. **This is the option taken.**

**What the decision was taken against, recorded because it bears on the work.**
The tick is already deterministic by construction, so the seeded scheduler has
no caller today. The `schedule()` function sets `SingleThreadedExecutor` explicitly and chains
its two systems. The `run_steps` function awaits each recorded effect in its turn. The one
concurrent phase, the bounded waves of source polls, documents that its results
"are collected in declaration order whatever order they resolve in."

Four tests
already hold that line. The test `ten_thousand_requests_replay_identically` drives 10,000
steps over 20 hosts and asserts identical verdicts *and* identical egress windows.
The test `permits_from_identically_driven_frontiers_compare_equal` states the same
determinism on the permit itself. The tests
`evidence_that_the_effect_happened_records_it_without_replaying_it` and
`an_attempt_whose_outcome_was_never_recorded_is_held_not_replayed` pin the replay
semantics that make a recorded run exact. So this adds a capability nothing
exercises until the recorder lands (`docs/general-bot-fold.md` §6, item 10),
which §12 names as a failure mode. It is taken deliberately, not overlooked, and
the recorder is what will exercise it.

One consequence to settle while building it: a seeded scheduler needs a userspace
deterministic PRNG, and `lgwks_std::random` enforces INV-RANDOM-ONE-SOURCE —
"never a silent fallback to a clock, a counter, or userspace PRNG". The carve-out
is that a replay device is not an entropy source, and it has to be argued in the
module rather than assumed at the call site. `lgwks_std::seeded` (#344,
INV-STD-SEEDED-1) is that device.

## 10. Migration order

Each step must leave the gate (`./scripts/ci-local.sh`) green.

0. **Finish getting the tree green.** ✅ Done. `cargo test --workspace
   --all-targets --locked` → 208 passed, 0 failed; clippy `-D warnings` clean;
   `cargo fmt --all -- --check` exit 0; `lgwks-deps check .` exit 0 with 27
   approvals; `lgwks-std-package-smoke.sh` passed.
1. **The scheduler decision** from §9: vendored zed `scheduler`, or an
   `lgwks_std` module. ✅ **Decided — the `lgwks_std` module**, taken by the
   project owner on 2026-09-21. Not yet built; §9 records both rungs and what
   the decision was taken against.
2. **A `Spec -> Schedule` executor.** ✅ Landed, and it *replaced* the old one
   rather than sitting beside it. `Bot` is the ECS bot: `SourceId`/`Revision`
   components, `Grants`/`Fired`/`TickError` resources, non-`Send` chain and value
   storage, the `observe_fold` and `fire_plan` exclusive systems, and bounded concurrent
   polling retained from the previous executor. The interim state (two bots, one
   behind a default-off `ecs` feature) was rejected as a candidate architecture
   that nothing would exercise; see §12.

   **Landed: the registry and `Bot::from_spec`.** The
   `domain_id -> constructor` registry (`DomainRegistry`, the `domains!` list)
   names the concrete `Source`/`Action` a document's strings resolve to. The call
   `Bot::from_spec(spec, registry, grants, effects)` materializes a runnable
   `Bot` through the same `assemble`/`build` path a native bot uses — no second
   interpreter and no second execution path, so the two produce the same
   operation trace. Admission is all-or-nothing: every presently knowable unmet
   need is reported in one attributed `NeedSet`, and a registry with a duplicate
   identifier is refused rather than dispatched by declaration order. See
   [the materializer](guides/lgwks-bot/domains.md);
   `experience/invariants/sdk.yaml` records it as landed.

   The crate doc used to argue that the absence was a design position rather than
   a gap. That reading was put to the project owner on 2026-09-21 and rejected:
   the ledger was right and the registry was to be built. Two constraints survive
   and bind the implementation, because they are what made the absence
   defensible in the first place. Grants still come from a `GrantSet` the caller
   holds and never from the spec, so wire data cannot choose what a bot reaches,
   and there is still no `bot!` proc-macro. This is also the same registry the
   `Lambda`/`Workers` comparison arrived at from the other direction.
3. **Build-time schedule validation**: ✅ landed with step 2, since it has
   nowhere else to live: `Schedule::initialize()` plus `ambiguity_detection:
   LogLevel::Error`, surfaced as `BotError` from `build()`, with a control test
   that the same two systems *ordered* do validate.
4. **`Time<Virtual>` clock root**, replacing wall-clock reads, with real and
   virtual implementations. ✅ **Closed as already satisfied**, 2026-09-21. Two
   time layers exist and both are tested. The first, `lgwks_std::time`, is pure RFC 3339
   format and parse plus calendar arithmetic over `SystemTime` (INV-TIME-PURE).
   The second, `lgwks_bot::rt::time`, supplies `sleep`, `sleep_until`, `timeout`,
   `interval`, `Instant` and `Elapsed`. #152 later added the declared
   `rt::clock::Clock`, virtual or wall, so a deadline names the clock that governs
   it (`docs/async-parity.md` §4.1). The settlement policy
   (`docs/general-bot-fold.md` §6, item 9) is still unbuilt.
5. **Effects move to exclusive systems**, and the deferred path is removed so
   §3's silent-no-op failure cannot recur. ✅ **Landed with step 2.** This list
   went on showing it as outstanding after the fact. Effects do not go through
   `Commands` (see the module doc on `auto_insert_apply_deferred`), and `observe_fold`
   and `fire_plan` are exclusive systems. The schedule adds only
   `(observe_fold, fire_plan).chain()` with no `ApplyDeferred`, and the effects a
   tick decided are awaited by `run_steps` outside the schedule.
6. **bors invariants land on the control plane**: derived readiness, one
   reconciler, checkpoint-after-effect, replay of recorded intent.

`Bot::tick()` stayed working through every step, and step 2 replaced the executor
behind it. Step 6 is not built.
