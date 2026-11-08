//! A scope's stop in the consumer's own journal, against a saboteur and a model.
//!
//! Source: a consumer's cancellation lifecycle (E30, R72, its issue 119): a requested
//! stop survives a restart, moves in one order, and never touches a neighbour.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use lgwks_bot::journal::RecordJournal;
use lgwks_bot::rt::sync::CancellationToken;
use lgwks_bot::script::{CancelError, CancelRecord, CancelStage, JournaledCancel, Tenant};
use lgwks_std::seeded::Seeded;
use lgwks_std::wire::rkyv::{Archive, Deserialize, Serialize};

use crate::sim::scratch_file::ScratchFile;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;
/// The journal, as a consumer shares it with its stops.
type Shared = Arc<Mutex<RecordJournal<Host>>>;

/// A consumer's record: its own work, or a stop moving.
#[derive(Archive, Serialize, Deserialize, Debug, Clone, PartialEq)]
#[rkyv(crate = lgwks_std::wire::rkyv)]
enum Host {
    /// The consumer's own work.
    Work(u64),
    /// A stop moved.
    Moved {
        /// Which stop.
        name: String,
        /// To where.
        stage: CancelStage,
    },
}

impl CancelRecord for Host {
    fn cancel(name: &str, stage: CancelStage) -> Self {
        Self::Moved {
            name: name.to_owned(),
            stage,
        }
    }

    fn as_cancel(&self) -> Option<(&str, CancelStage)> {
        if let Self::Moved { ref name, stage } = *self {
            return Some((name.as_str(), stage));
        }
        None
    }
}

/// The journal at `scratch`, opened as a restart opens it.
fn open(scratch: &ScratchFile) -> Outcome<Shared> {
    Ok(Arc::new(Mutex::new(RecordJournal::open(scratch.path())?)))
}

/// The stop `name` over `journal`, with a fresh token.
fn stop(journal: &Shared, name: &str) -> Result<JournaledCancel<Host>, CancelError> {
    JournaledCancel::restore(Arc::clone(journal), name, CancellationToken::new())
}

/// Every record the journal holds.
fn records(journal: &Shared) -> Outcome<Vec<Host>> {
    let held = journal.lock().map_err(|cause| cause.to_string())?;
    let mut all = Vec::new();
    for entry in held.replay()? {
        all.push(entry?.record().clone());
    }
    Ok(all)
}

/// Write `record` straight into the journal, as a saboteur would.
fn plant(journal: &Shared, record: &Host) -> Outcome {
    let mut held = journal.lock().map_err(|cause| cause.to_string())?;
    let tail = held.tail();
    held.compare_and_append(tail, record)?;
    Ok(())
}

/// Catches a stop that a restart forgets, at every stage it can stand at.
#[test]
fn sim_journaled_cancel_pulled_plug_every_stage_survives_a_restart() -> Outcome {
    let scratch = ScratchFile::new("cancel-restart")?;
    let tenant = Tenant::new("acme")?;
    let never = stop(&open(&scratch)?, "run")?;
    assert_eq!(never.stage(), None);
    assert!(!never.token().is_cancelled());
    assert!(never.scope(tenant.clone()).enter("step").is_ok());
    drop(never);

    for (step, wanted) in [
        ("request", CancelStage::Requested),
        ("begin", CancelStage::Cancelling),
        ("finish", CancelStage::Finished),
    ] {
        let journal = open(&scratch)?;
        let live = stop(&journal, "run")?;
        match step {
            "request" => live.request()?,
            "begin" => live.begin()?,
            _ => live.finish()?,
        };
        drop((live, journal));
        let again = stop(&open(&scratch)?, "run")?;
        assert_eq!(again.stage(), Some(wanted), "after {step} and a restart");
        assert!(again.token().is_cancelled(), "after {step} and a restart");
        assert!(again.scope(tenant.clone()).enter("step").is_err());
    }
    Ok(())
}

/// Catches a stage written out of order, and a refused one reaching the disk.
#[test]
fn sim_journaled_cancel_wrong_order_refused_stages_write_nothing() -> Outcome {
    let scratch = ScratchFile::new("cancel-order")?;
    let journal = open(&scratch)?;
    let live = stop(&journal, "run")?;
    assert!(matches!(
        live.begin(),
        Err(CancelError::OutOfOrder {
            held: None,
            asked: CancelStage::Cancelling
        })
    ));
    assert!(matches!(live.finish(), Err(CancelError::OutOfOrder { .. })));
    assert!(
        !live.token().is_cancelled(),
        "a refused stage stops nothing"
    );
    assert_eq!(records(&journal)?, []);
    live.request()?;
    assert!(matches!(
        live.finish(),
        Err(CancelError::OutOfOrder {
            held: Some(CancelStage::Requested),
            ..
        })
    ));
    assert_eq!(records(&journal)?.len(), 1);
    Ok(())
}

/// Catches a repeat writing a second record, and a late stage moving a stop back.
#[test]
fn sim_journaled_cancel_double_clicker_asking_twice_records_once() -> Outcome {
    let scratch = ScratchFile::new("cancel-twice")?;
    let journal = open(&scratch)?;
    let live = stop(&journal, "run")?;
    live.request()?;
    assert_eq!(live.request()?, CancelStage::Requested);
    live.begin()?;
    assert_eq!(live.begin()?, CancelStage::Cancelling);
    assert_eq!(live.request()?, CancelStage::Cancelling, "never back");
    live.finish()?;
    assert_eq!(live.finish()?, CancelStage::Finished);
    assert_eq!(records(&journal)?.len(), 3);
    Ok(())
}

/// Catches two stops sharing a fate, in one journal with the consumer's own work.
#[test]
fn sim_journaled_cancel_neighbour_one_stop_leaves_the_others_running() -> Outcome {
    let scratch = ScratchFile::new("cancel-neighbour")?;
    let journal = open(&scratch)?;
    let stops: Vec<JournaledCancel<Host>> = (0..200)
        .map(|number| stop(&journal, &format!("run-{number}")))
        .collect::<Result<_, _>>()?;
    for (number, one) in stops.iter().enumerate() {
        plant(&journal, &Host::Work(u64::try_from(number)?))?;
        if number % 10 == 0 {
            one.request()?;
        }
    }
    drop((stops, journal));
    let reopened = open(&scratch)?;
    for number in 0..200 {
        let one = stop(&reopened, &format!("run-{number}"))?;
        assert_eq!(one.token().is_cancelled(), number % 10 == 0, "run-{number}");
    }
    Ok(())
}

/// Catches a restart believing a finish nobody asked for.
#[test]
fn sim_journaled_cancel_saboteur_a_planted_stage_is_refused_at_restore() -> Outcome {
    for planted in [
        vec![CancelStage::Finished],
        vec![CancelStage::Cancelling],
        vec![CancelStage::Requested, CancelStage::Finished],
        vec![
            CancelStage::Requested,
            CancelStage::Cancelling,
            CancelStage::Requested,
        ],
    ] {
        let scratch = ScratchFile::new("cancel-saboteur")?;
        let journal = open(&scratch)?;
        for stage in &planted {
            plant(&journal, &Host::cancel("run", *stage))?;
        }
        assert!(
            matches!(stop(&journal, "run"), Err(CancelError::Diverged { .. })),
            "{planted:?}"
        );
        assert!(
            stop(&journal, "other").is_ok(),
            "a neighbour is not refused"
        );
    }
    let scratch = ScratchFile::new("cancel-repeat")?;
    let journal = open(&scratch)?;
    plant(&journal, &Host::cancel("run", CancelStage::Requested))?;
    plant(&journal, &Host::cancel("run", CancelStage::Requested))?;
    assert_eq!(stop(&journal, "run")?.stage(), Some(CancelStage::Requested));
    Ok(())
}

/// Catches a full journal holding back a stop, and a refusal that reads as recorded.
#[test]
fn sim_journaled_cancel_giant_and_empty_a_full_journal_still_stops_this_process() -> Outcome {
    let scratch = ScratchFile::new("cancel-full")?;
    let journal = open(&scratch)?;
    {
        let mut held = journal.lock().map_err(|cause| cause.to_string())?;
        let batch: Vec<Host> = (0..100_000).map(Host::Work).collect();
        let tail = held.tail();
        held.compare_and_append_all(tail, &batch)?;
    }
    let live = stop(&journal, "run")?;
    assert!(matches!(live.request(), Err(CancelError::Journal(_))));
    assert!(live.token().is_cancelled(), "it stops here all the same");
    assert_eq!(live.stage(), None, "and it does not claim a record");
    assert!(matches!(live.request(), Err(CancelError::Journal(_))));
    Ok(())
}

/// Catches two callers racing a request into two records.
#[test]
fn sim_journaled_cancel_double_clicker_a_hundred_threads_record_one_request() -> Outcome {
    let scratch = ScratchFile::new("cancel-race")?;
    let journal = open(&scratch)?;
    let live = stop(&journal, "run")?;
    std::thread::scope(|threads| {
        for _ in 0..100 {
            let clone = live.clone();
            threads.spawn(move || clone.request().is_ok());
        }
    });
    assert_eq!(
        records(&journal)?,
        [Host::cancel("run", CancelStage::Requested)]
    );
    Ok(())
}

/// One seed's random moves, restarts included, held to a model, as a trace.
fn monkey(seed: u64) -> Outcome<Vec<(usize, Option<CancelStage>)>> {
    let mut dice = Seeded::from_seed(seed);
    let scratch = ScratchFile::new("cancel-monkey")?;
    let mut journal = open(&scratch)?;
    let mut model: BTreeMap<usize, Option<CancelStage>> = BTreeMap::new();
    let mut trace = Vec::new();
    for _ in 0..80 {
        let which = usize::try_from(dice.below(4)?)?;
        let name = format!("run-{which}");
        let held = *model.entry(which).or_default();
        let one = stop(&journal, &name)?;
        assert_eq!(one.stage(), held, "seed {seed} read a stage back wrong");
        assert_eq!(one.token().is_cancelled(), held.is_some(), "seed {seed}");
        let (asked, answer) = match dice.below(4)? {
            0 => (CancelStage::Requested, one.request()),
            1 => (CancelStage::Cancelling, one.begin()),
            2 => (CancelStage::Finished, one.finish()),
            _ => {
                drop((one, journal));
                journal = open(&scratch)?;
                continue;
            }
        };
        let expected = match (held, asked) {
            (Some(at), wanted) if at >= wanted => Some(at),
            (None, CancelStage::Requested)
            | (Some(CancelStage::Requested), CancelStage::Cancelling)
            | (Some(CancelStage::Cancelling), CancelStage::Finished) => Some(asked),
            _ => None,
        };
        match expected {
            Some(stage) => {
                assert_eq!(answer?, stage, "seed {seed}");
                model.insert(which, Some(stage));
            }
            None => assert!(answer.is_err(), "seed {seed}: {held:?} to {asked:?}"),
        }
        trace.push((which, model.get(&which).copied().flatten()));
    }
    Ok(trace)
}

/// Catches any drift from the model, and any order a scheduler could change.
#[test]
fn sim_journaled_cancel_monkey_random_moves_match_the_model_and_replay_exactly() -> Outcome {
    for seed in 0..48 {
        assert_eq!(monkey(seed)?, monkey(seed)?, "seed {seed} did not replay");
    }
    Ok(())
}
