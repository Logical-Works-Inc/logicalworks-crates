# ADR 0017: the broker's fence for what a generation does not answer

Status: **accepted**, 2026-11-08. Implements #876.

Stakeholders: the Director; agent-harness authors (Larry); `lgwks_bot` maintainers.
Concerns: a stale owner reaching after a takeover; an operation that must stop now while its siblings go on; authority that outlives contact with its source; a takeover over a consumer's own history.
Requirements: R9, R10
Viewpoint: the authority viewpoint, mapping what a host learns to which reaches are still allowed.
Elements and relations: `lgwks_bot::broker::Broker` with `authenticate`, `revoke_operation`, `set_offline_bound`, `spend` and `adopt_records`; `Authority::action`; the dispatch path in `ecs::bot_run`; `RecordJournal` from ADR 0016.

## Context

The broker answered one question: was this attempt prepared against the current generation? A harness asks three more. The authority source reports a generation the broker has not reached, and stepping `replace` up to a `u64` is unbounded. One operation must stop while the others carry on. A process that has lost its authority source must not go on reaching forever, and no clock reading can settle that because a clock can be set back.

## Decision

1. **Raise in one step, never back.** `authenticate` moves to the later of what it holds and what was reported, so a replayed older snapshot is absorbed.
2. **Withdraw by name, one way.** `revoke_operation` has no inverse. Restoring authority is a new authorization. At most 65,536 are held per environment, and past that the refusal says so while `replace` still fences everything below it.
3. **A count, not a duration.** `set_offline_bound` caps reaches between contacts, `spend` counts one, and only `authenticate` clears the count. A bound of zero stays shut through authentication, so it is a kill switch.
4. **The check and the count are one call.** `spend` re-checks the warrant before counting. The dispatch path calls it where it used to revalidate, so every handed-over attempt spends, a retry included.
5. **One refusal order.** Unknown, closed, revoked, never issued, superseded, spent. An attempt that fails several checks always reports the same reason.
6. **One takeover.** `adopt` and `adopt_records` share a private function, so the highest generation named plus one is the rule for both. `epoch_of` returns `None` for a record about something else.
7. **Observation is not fenced.** Refusing a status query would leave an effect already sent permanently unlabelled.

## Alternatives rejected

- **A lease with an expiry time.** A time bound can be extended by a clock set back and can lapse under a healthy process. Reaches are what the bound exists to limit, and they can be counted exactly.
- **Looping `replace` to a reported generation.** One forged report near `u64::MAX` would take 2^64 steps.
- **Journaling revocations and the count inside the broker.** The broker is live state that a host rebuilds. A host that must not recharge across a restart records its count in its own journal and sets the bound from it.
- **Making the journal generic so `adopt` reads either.** `adopt_records` keeps the consumer's decoding in a closure, and `epoch_of` is the only thing the broker must learn about its records.

## Consequences

- A restart recharges the offline allowance unless the host re-establishes it, as stated in the module docs. Withdrawals are also live state, so a host re-applies them after a restart.
- `adopt_records` reads the live generation only, so a consumer that continues its journal must carry the highest generation in the checkpoint. `sim_broker_fence_pulled_plug_a_checkpoint_carries_the_generation_across_a_seal` holds that shape.
- A running `Bot` owns its broker, so a host cannot authenticate during a run through it. A bound on a `Bot` is therefore a total for that run until the bot is rebuilt.
- A consumer whose generations start at zero maps them to `EnvironmentEpoch` by adding one, since this type is non-zero.

## Correspondence

Builds on ADR 0016's `RecordJournal`.

## Verification

`broker-fence` is the journey. `tests/it/broker_fence_cli.rs` drives it through a real kill, `tests/it/broker_fence_dispatch.rs` holds the bound on a running bot, and `tests/it/sim_broker_fence.rs` holds the fence against a reference model over 256 seeds.
