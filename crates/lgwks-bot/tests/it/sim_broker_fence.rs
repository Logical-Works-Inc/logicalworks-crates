//! The broker's fence, driven through its public surface by fools and a model.
//!
//! Source: a consumer's revocation fence and its epoch simulations (E71, R87, D48),
//! ported from a runtime onto `Broker`, plus the Gap 2 issue's four requirements.
//! The oracle is a reference model written beside the broker, never the broker's
//! own output, and every seeded family replays to one trace.

use std::collections::{BTreeMap, BTreeSet};

use core::num::NonZeroU64;
use lgwks_bot::broker::{Authority, Broker, BrokerError};
use lgwks_bot::effect::{
    ActionDigest, ActionId, AttemptId, EffectIdentity, EffectKey, EnvironmentEpoch, EnvironmentId,
    FlowRevision, Id128, RunId,
};
use lgwks_bot::journal::{ContinuationPolicy, JournalError, RecordJournal};
use lgwks_std::seeded::Seeded;

use crate::sim::scratch_file::ScratchFile;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A non-zero identifier from a small number.
fn id128(value: u128) -> Outcome<Id128> {
    Ok(Id128::from_nonzero(
        core::num::NonZeroU128::new(value).ok_or("identifiers are not zero")?,
    ))
}

/// The environment numbered `value`.
fn environment(value: u128) -> Outcome<EnvironmentId> {
    Ok(EnvironmentId::new(id128(value)?))
}

/// The operation numbered `value`.
fn operation(value: u128) -> Outcome<ActionId> {
    Ok(ActionId::new(id128(value)?))
}

/// The generation numbered `value`.
fn at(value: u64) -> Outcome<EnvironmentEpoch> {
    Ok(EnvironmentEpoch::new(
        NonZeroU64::new(value).ok_or("generation zero was never issued")?,
    ))
}

/// A key for `action` on `id` at generation `epoch`.
fn key(id: u128, action: u128, epoch: u64) -> Outcome<EffectKey> {
    let hex = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    let run = RunId::new(id128(0xABC)?);
    let flow = FlowRevision::from_tagged("blake3_256", hex)?;
    let identity = EffectIdentity::new(run, environment(id)?, flow);
    let attempt = AttemptId::from_decimal("1")?;
    let digest = ActionDigest::from_tagged("blake3_256", hex)?;
    Ok(identity.key(operation(action)?, attempt, digest, at(epoch)?))
}

/// A broker holding environment 1 at its first generation, with `bound` reaches.
fn fenced(bound: u32) -> Outcome<Broker> {
    let mut broker = Broker::new();
    broker.register(environment(1)?)?;
    broker.set_offline_bound(environment(1)?, bound)?;
    Ok(broker)
}

/// Mint and spend one warrant, the whole of a mutating reach.
fn reach(broker: &mut Broker, action: u128, epoch: u64) -> Outcome<Result<(), BrokerError>> {
    Ok(match broker.authorize(key(1, action, epoch)?) {
        Ok(warrant) => broker.spend(&warrant),
        Err(refusal) => Err(refusal),
    })
}

/// Catches a first reach refused by a fence nothing has touched.
#[test]
fn sim_broker_fence_wrong_order_a_fresh_fence_admits_the_reach() -> Outcome {
    let mut broker = fenced(4)?;
    assert!(reach(&mut broker, 1, 1)?.is_ok());
    assert_eq!(broker.offline_spent(environment(1)?), Some(1));
    assert!(!broker.is_revoked(environment(1)?, operation(1)?));
    Ok(())
}

/// Catches a revocation that a second call, a move or a report can undo, and one
/// that spreads to a sibling.
#[test]
fn sim_broker_fence_double_clicker_revocation_is_one_way_and_touches_no_sibling() -> Outcome {
    let (id, seven) = (environment(1)?, operation(7)?);
    let mut broker = fenced(64)?;
    assert!(broker.revoke_operation(id, seven)?, "the first withdraws");
    assert!(
        !broker.revoke_operation(id, seven)?,
        "the second is a no-op"
    );
    assert!(matches!(
        reach(&mut broker, 7, 1)?,
        Err(BrokerError::Revoked { action, .. }) if action == seven
    ));
    assert!(reach(&mut broker, 8, 1)?.is_ok(), "not a blanket");
    broker.replace(id)?;
    assert_eq!(
        broker.authenticate(id, at(1)?)?,
        at(2)?,
        "the replay is absorbed"
    );
    assert!(broker.is_revoked(id, seven), "and the withdrawal stands");
    assert!(matches!(
        reach(&mut broker, 7, 2)?,
        Err(BrokerError::Revoked { .. })
    ));
    Ok(())
}

/// Catches a generation move that fences everything, or that fences nothing.
#[test]
fn sim_broker_fence_wrong_order_a_move_fences_only_what_it_left_behind() -> Outcome {
    let mut broker = fenced(64)?;
    let stranded = broker.authorize(key(1, 1, 1)?)?;
    broker.replace(environment(1)?)?;
    assert!(matches!(
        broker.spend(&stranded),
        Err(BrokerError::Superseded { .. })
    ));
    assert!(
        reach(&mut broker, 2, 2)?.is_ok(),
        "the new generation works"
    );
    Ok(())
}

/// Catches an authentication that lowers the fence, moves it stepwise, or leaves the
/// count standing.
#[test]
fn sim_broker_fence_saboteur_a_replayed_older_report_is_absorbed() -> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(4)?;
    assert!(reach(&mut broker, 1, 1)?.is_ok());
    assert_eq!(
        broker.authenticate(id, at(u64::MAX)?)?,
        at(u64::MAX)?,
        "one step"
    );
    assert_eq!(
        broker.offline_spent(id),
        Some(0),
        "reaching the source refreshes"
    );
    assert_eq!(
        broker.authenticate(id, at(2)?)?,
        at(u64::MAX)?,
        "never back"
    );
    assert_eq!(broker.epoch(id), Some(at(u64::MAX)?));
    assert!(matches!(
        reach(&mut broker, 1, u64::MAX - 1)?,
        Err(BrokerError::Superseded { .. })
    ));
    assert!(matches!(
        broker.replace(id),
        Err(BrokerError::Exhausted { .. })
    ));
    assert_eq!(
        broker.epoch(id),
        Some(at(u64::MAX)?),
        "a refusal moves nothing"
    );
    Ok(())
}

/// Catches a bound recharged by anything but the source. No call here takes a time,
/// so neither a sleep nor a clock set back can reach it: only these calls can try.
#[test]
fn sim_broker_fence_liar_upstream_only_authenticating_recharges_the_bound() -> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(1)?;
    assert!(reach(&mut broker, 1, 1)?.is_ok());
    let spent = || {
        Err::<(), _>(BrokerError::OfflineExposureExhausted {
            environment: id,
            bound: 1,
        })
    };
    assert_eq!(
        format!("{:?}", reach(&mut broker, 2, 1)?),
        format!("{:?}", spent())
    );
    broker.set_offline_bound(id, 1)?;
    assert_eq!(
        format!("{:?}", reach(&mut broker, 2, 1)?),
        format!("{:?}", spent()),
        "setting it again"
    );
    let moved = broker.replace(id)?;
    assert_eq!(
        broker.offline_spent(id),
        Some(1),
        "a local move is no contact"
    );
    assert!(reach(&mut broker, 2, moved.get())?.is_err());
    broker.authenticate(id, moved)?;
    assert!(reach(&mut broker, 2, moved.get())?.is_ok(), "recharged");
    assert!(
        reach(&mut broker, 3, moved.get())?.is_err(),
        "by one bound's worth"
    );
    Ok(())
}

/// Catches zero read as "authenticate first": it stays shut, and the count clears.
#[test]
fn sim_broker_fence_giant_and_empty_a_zero_bound_is_a_kill_switch() -> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(0)?;
    assert!(matches!(
        reach(&mut broker, 1, 1)?,
        Err(BrokerError::OfflineExposureExhausted { bound: 0, .. })
    ));
    broker.authenticate(id, at(1)?)?;
    assert_eq!(broker.offline_spent(id), Some(0));
    assert!(matches!(
        reach(&mut broker, 1, 1)?,
        Err(BrokerError::OfflineExposureExhausted { bound: 0, .. })
    ));
    Ok(())
}

/// Catches an off-by-one at the ceiling, and a refused spend that still counts.
#[test]
fn sim_broker_fence_giant_and_empty_every_bound_admits_exactly_that_many() -> Outcome {
    for bound in [1_u32, 2, 3, 64, 1_024] {
        let mut broker = fenced(bound)?;
        let warrant = broker.authorize(key(1, 1, 1)?)?;
        for _ in 0..bound {
            broker.spend(&warrant)?;
        }
        assert!(broker.spend(&warrant).is_err());
        assert!(broker.spend(&warrant).is_err());
        assert_eq!(broker.offline_spent(environment(1)?), Some(bound));
    }
    Ok(())
}

/// Catches a bound of `u32::MAX` read as a wrap, and a broker with no bound counting.
#[test]
fn sim_broker_fence_giant_and_empty_no_bound_counts_nothing_and_the_largest_admits() -> Outcome {
    let id = environment(1)?;
    let mut free = Broker::new();
    free.register(id)?;
    let warrant = free.authorize(key(1, 1, 1)?)?;
    free.spend(&warrant)?;
    assert_eq!(free.offline_spent(id), None);
    assert_eq!(free.offline_bound(id), None);
    let mut large = fenced(u32::MAX)?;
    let warrant = large.authorize(key(1, 1, 1)?)?;
    large.spend(&warrant)?;
    assert_eq!(large.offline_spent(id), Some(1));
    Ok(())
}

/// Catches an unbounded revocation set, and a full set that stops answering.
#[test]
fn sim_broker_fence_giant_and_empty_the_withdrawal_ceiling_binds_and_the_move_still_works()
-> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(u32::MAX)?;
    for value in 1..=65_536_u128 {
        assert!(broker.revoke_operation(id, operation(value)?)?);
    }
    assert!(matches!(
        broker.revoke_operation(id, operation(65_537)?),
        Err(BrokerError::RevocationsFull { limit: 65_536, .. })
    ));
    assert!(
        !broker.revoke_operation(id, operation(1)?)?,
        "an old name still answers"
    );
    assert!(
        !broker.is_revoked(id, operation(65_537)?),
        "a refusal withdrew nothing"
    );
    let stranded = broker.authorize(key(1, 65_537, 1)?)?;
    broker.replace(id)?;
    assert!(
        broker.spend(&stranded).is_err(),
        "the wider withdrawal still fences it"
    );
    Ok(())
}

/// Catches one attempt reporting different reasons as facts pile up.
#[test]
fn sim_broker_fence_wrong_order_refusals_run_revoked_then_stale_then_spent() -> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(0)?;
    broker.revoke_operation(id, operation(3)?)?;
    broker.replace(id)?;
    assert!(matches!(
        broker.authorize(key(1, 3, 1)?),
        Err(BrokerError::Revoked { .. })
    ));
    assert!(matches!(
        broker.authorize(key(1, 4, 1)?),
        Err(BrokerError::Superseded { .. })
    ));
    assert!(matches!(
        broker.authorize(key(1, 5, 2)?),
        Err(BrokerError::OfflineExposureExhausted { bound: 0, .. })
    ));
    Ok(())
}

/// Catches a withdrawal or a spent bound that a warrant already minted walks past.
#[test]
fn sim_broker_fence_wrong_order_a_warrant_minted_earlier_is_refused_at_the_reach() -> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(1)?;
    let withdrawn: Authority = broker.authorize(key(1, 1, 1)?)?;
    assert_eq!(withdrawn.action(), operation(1)?);
    broker.revoke_operation(id, operation(1)?)?;
    assert!(matches!(
        broker.revalidate(&withdrawn),
        Err(BrokerError::Revoked { .. })
    ));
    assert!(matches!(
        broker.spend(&withdrawn),
        Err(BrokerError::Revoked { .. })
    ));
    assert_eq!(
        broker.offline_spent(id),
        Some(0),
        "a refusal spends nothing"
    );
    Ok(())
}

/// Catches calls after release reviving an environment, and a late withdrawal lost.
#[test]
fn sim_broker_fence_wrong_order_a_closed_environment_mints_nothing_and_still_records() -> Outcome {
    let id = environment(1)?;
    let mut broker = fenced(4)?;
    let warrant = broker.authorize(key(1, 1, 1)?)?;
    broker.close(id)?;
    assert!(matches!(
        broker.spend(&warrant),
        Err(BrokerError::Closed { .. })
    ));
    assert!(matches!(
        broker.authenticate(id, at(9)?),
        Err(BrokerError::Closed { .. })
    ));
    assert!(
        broker.revoke_operation(id, operation(2)?)?,
        "a late withdrawal stands"
    );
    assert_eq!(broker.epoch(id), Some(at(1)?));
    let nobody = environment(9)?;
    assert!(matches!(
        broker.authenticate(nobody, at(1)?),
        Err(BrokerError::UnknownEnvironment { .. })
    ));
    assert!(matches!(
        broker.revoke_operation(nobody, operation(1)?),
        Err(BrokerError::UnknownEnvironment { .. })
    ));
    assert!(matches!(
        broker.set_offline_bound(nobody, 1),
        Err(BrokerError::UnknownEnvironment { .. })
    ));
    assert!(!broker.is_revoked(nobody, operation(1)?));
    assert_eq!(broker.offline_spent(nobody), None);
    Ok(())
}

/// Catches one environment's fence reaching another's, at many tenants at once.
#[test]
fn sim_broker_fence_neighbour_ten_thousand_environments_keep_their_own_fence() -> Outcome {
    let mut broker = Broker::new();
    for value in 1..=10_000_u128 {
        broker.register(environment(value)?)?;
        broker.set_offline_bound(environment(value)?, 1)?;
    }
    let target = environment(5_000)?;
    broker.revoke_operation(target, operation(1)?)?;
    broker.authenticate(target, at(7)?)?;
    let warrant = broker.authorize(key(4_999, 1, 1)?)?;
    broker.spend(&warrant)?;
    for value in [1_u128, 4_998, 5_001, 10_000] {
        let id = environment(value)?;
        assert!(!broker.is_revoked(id, operation(1)?));
        assert_eq!(broker.epoch(id), Some(at(1)?));
        assert_eq!(broker.offline_spent(id), Some(0));
    }
    assert_eq!(broker.offline_spent(environment(4_999)?), Some(1));
    assert!(matches!(
        broker.authorize(key(5_000, 1, 7)?),
        Err(BrokerError::Revoked { .. })
    ));
    Ok(())
}

/// What the journal in these takeovers names: a generation of environment 1, a
/// record about nothing, or another environment's generation.
fn named_by(record: &u64) -> Option<(EnvironmentId, EnvironmentEpoch)> {
    let (who, generation) = (*record >> 32, *record & 0xFFFF_FFFF);
    let id = environment(u128::from(who)).ok()?;
    Some((id, at(generation).ok()?))
}

/// A record naming `generation` of environment `who`.
const fn names(who: u64, generation: u64) -> u64 {
    (who << 32) | generation
}

/// Catches a takeover that trusts the last record, or starts over at generation one.
#[test]
fn sim_broker_fence_pulled_plug_adoption_moves_past_the_highest_named() -> Outcome {
    let scratch = ScratchFile::new("broker-sim-adopt")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    for record in [names(1, 3), 0, names(1, 9), names(1, 4)] {
        journal.compare_and_append(journal.tail(), &record)?;
    }
    drop(journal);
    let journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut broker = Broker::new();
    let id = environment(1)?;
    assert_eq!(broker.adopt_records(id, &journal, named_by)?, at(10)?);
    assert!(matches!(
        broker.authorize(key(1, 1, 9)?),
        Err(BrokerError::Superseded { .. })
    ));
    assert!(broker.authorize(key(1, 1, 10)?).is_ok());
    Ok(())
}

/// Catches a takeover over nothing, a foreign history, a held environment or rotted
/// bytes that registers anything at all.
#[test]
fn sim_broker_fence_saboteur_a_refused_takeover_leaves_nothing_behind() -> Outcome {
    let id = environment(1)?;
    let scratch = ScratchFile::new("broker-sim-refuse")?;
    let mut journal = RecordJournal::<u64>::open(scratch.path())?;
    let mut broker = Broker::new();
    assert!(matches!(
        broker.adopt_records(id, &journal, named_by),
        Err(BrokerError::NothingToAdopt { .. })
    ));
    journal.compare_and_append(journal.tail(), &0)?;
    assert!(matches!(
        broker.adopt_records(id, &journal, named_by),
        Err(BrokerError::NothingToAdopt { .. })
    ));
    journal.compare_and_append(journal.tail(), &names(2, 3))?;
    assert!(matches!(
        broker.adopt_records(id, &journal, named_by),
        Err(BrokerError::ForeignEnvironment { .. })
    ));
    assert_eq!(broker.epoch(id), None, "a refusal claims nothing");
    let tail = journal.tail();
    let last = journal.compare_and_append(tail, &names(1, u64::from(u32::MAX)))?;
    assert_eq!(last.position().sequence(), 3);
    let mut held = fenced(1)?;
    assert!(matches!(
        held.adopt_records(id, &journal, named_by),
        Err(BrokerError::AlreadyRegistered { .. })
    ));
    drop(journal);

    let mut bytes = std::fs::read(scratch.path())?;
    let middle = bytes.len() >> 1;
    let byte = bytes.get_mut(middle).ok_or("the journal holds bytes")?;
    *byte ^= 0x40;
    std::fs::write(scratch.path(), &bytes)?;
    assert!(matches!(
        RecordJournal::<u64>::open(scratch.path()),
        Err(JournalError::Corrupt(_))
    ));
    Ok(())
}

/// Catches a takeover moving backwards when a journal continued: only the live
/// generation is read, so its checkpoint carries the fence.
#[test]
fn sim_broker_fence_pulled_plug_a_checkpoint_carries_the_generation_across_a_seal() -> Outcome {
    let scratch = ScratchFile::new("broker-sim-seal")?;
    let policy = ContinuationPolicy::declared();
    let mut journal = RecordJournal::<u64>::open_continuing(scratch.path(), policy)?;
    journal.compare_and_append(journal.tail(), &names(1, 41))?;
    let successor = journal
        .continue_with(&names(1, 41))?
        .ok_or("this journal continues")?;
    let mut broker = Broker::new();
    assert_eq!(
        broker.adopt_records(environment(1)?, &successor, named_by)?,
        at(42)?
    );
    Ok(())
}

/// One environment in the reference model.
#[derive(Default)]
struct ModelEnvironment {
    /// The generation it stands at.
    epoch: u64,
    /// Whether it was closed.
    closed: bool,
    /// What was withdrawn by name.
    revoked: BTreeSet<u128>,
    /// The ceiling and the count, when a bound was set.
    offline: Option<(u32, u32)>,
}

/// The reference model: what one attempt must meet, in the documented order.
fn model_refusal(
    model: &BTreeMap<u128, ModelEnvironment>,
    id: u128,
    action: u128,
    epoch: u64,
) -> &'static str {
    let Some(held) = model.get(&id) else {
        return "unknown";
    };
    if held.closed {
        "closed"
    } else if held.revoked.contains(&action) {
        "revoked"
    } else if epoch > held.epoch {
        "never_issued"
    } else if epoch < held.epoch {
        "superseded"
    } else if matches!(held.offline, Some((bound, spent)) if spent >= bound) {
        "spent"
    } else {
        "ok"
    }
}

/// The broker's answer in the model's words.
fn word<T>(answer: &Result<T, BrokerError>) -> &'static str {
    match *answer {
        Ok(_) => "ok",
        Err(BrokerError::UnknownEnvironment { .. }) => "unknown",
        Err(BrokerError::Closed { .. }) => "closed",
        Err(BrokerError::Revoked { .. }) => "revoked",
        Err(BrokerError::NeverIssued { .. }) => "never_issued",
        Err(BrokerError::Superseded { .. }) => "superseded",
        Err(BrokerError::OfflineExposureExhausted { .. }) => "spent",
        Err(_) => "other",
    }
}

/// Every operation a seed draws, checked against the model, as a trace.
fn monkey(seed: u64) -> Outcome<Vec<String>> {
    let mut dice = Seeded::from_seed(seed);
    let mut broker = Broker::new();
    let mut model: BTreeMap<u128, ModelEnvironment> = BTreeMap::new();
    let mut trace = Vec::new();
    for _ in 0..600 {
        let who = u128::from(dice.below(3)?).saturating_add(1);
        let id = environment(who)?;
        let action = u128::from(dice.below(4)?).saturating_add(1);
        let number = dice.below(6)?.saturating_add(1);
        let held = model.get_mut(&who);
        let said = match dice.below(8)? {
            0 => {
                let answer = word(&broker.register(id));
                if held.is_none() {
                    assert_eq!(answer, "ok", "a new environment registers");
                    model.insert(
                        who,
                        ModelEnvironment {
                            epoch: 1,
                            ..ModelEnvironment::default()
                        },
                    );
                }
                answer
            }
            1 => {
                let answer = word(&broker.replace(id));
                if let Some(held) = held.filter(|held| !held.closed) {
                    held.epoch = held.epoch.saturating_add(1);
                }
                answer
            }
            2 => {
                let answer = word(&broker.authenticate(id, at(number)?));
                if let Some(held) = held.filter(|held| !held.closed) {
                    held.epoch = held.epoch.max(number);
                    if let Some(ref mut offline) = held.offline {
                        offline.1 = 0;
                    }
                }
                answer
            }
            3 => {
                let answer = broker.revoke_operation(id, operation(action)?);
                if let Some(held) = held {
                    assert_eq!(
                        answer.as_ref().ok().copied(),
                        Some(held.revoked.insert(action)),
                        "seed {seed}"
                    );
                }
                word(&answer)
            }
            4 => {
                let bound = u32::try_from(dice.below(4)?)?;
                let answer = word(&broker.set_offline_bound(id, bound));
                if let Some(held) = held {
                    let spent = held.offline.map(|offline| offline.1);
                    held.offline = Some((bound, spent.into_iter().sum()));
                }
                answer
            }
            5 if dice.below(40)? == 0 => {
                let answer = word(&broker.close(id));
                if let Some(held) = held {
                    held.closed = true;
                }
                answer
            }
            _ => {
                let answer = broker.authorize(key(who, action, number)?);
                let spoken = word(&answer);
                assert_eq!(
                    spoken,
                    model_refusal(&model, who, action, number),
                    "seed {seed}"
                );
                if let Ok(warrant) = answer {
                    broker.spend(&warrant)?;
                    let offline = model.get_mut(&who).and_then(|held| held.offline.as_mut());
                    if let Some(offline) = offline {
                        offline.1 = offline.1.saturating_add(1);
                    }
                }
                spoken
            }
        };
        if let Some(held) = model.get(&who) {
            assert_eq!(
                broker.epoch(id).map(|epoch| epoch.get()),
                Some(held.epoch),
                "seed {seed}"
            );
            assert_eq!(broker.is_open(id), !held.closed, "seed {seed}");
            assert_eq!(
                broker.offline_spent(id),
                held.offline.map(|offline| offline.1),
                "seed {seed}"
            );
        }
        trace.push(format!("{who}:{action}:{number}:{said}"));
    }
    Ok(trace)
}

/// Catches any drift between the broker and the documented fence, on every door.
#[test]
fn sim_broker_fence_monkey_random_ops_match_the_model_and_replay_exactly() -> Outcome {
    let mut traces = BTreeSet::new();
    for seed in 0..256 {
        let first = monkey(seed)?;
        assert_eq!(first, monkey(seed)?, "seed {seed} did not replay");
        traces.insert(first);
    }
    assert!(
        traces.len() > 200,
        "the seeds barely differ: {}",
        traces.len()
    );
    Ok(())
}
