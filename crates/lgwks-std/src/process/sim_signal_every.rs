//! Seeded simulations of the tree-stop sweep against a model.
//!
//! Source: #913 item 1. `kill_process_group` on Windows and the Unix orphan
//! reap both returned at the first refusal, so one descendant that answered
//! access-denied (`EPERM`, `ERROR_ACCESS_DENIED`) left every later descendant,
//! and on Windows the root, running. The model is the contract written down:
//! every target is signalled exactly once, in order, whatever earlier targets
//! answered, and the answer is the first refusal or `Ok`. One seed draws the
//! target count (0 to 256, the empty tree included) and which targets refuse.

use std::io;

use super::signal_every;
use crate::seeded::{DrawError, Seeded};

/// What a test returns: any failure, named.
type Outcome = Result<(), Box<dyn std::error::Error>>;

/// Seeds per family.
const SEEDS: u64 = 1_024;

/// Target ids and, per target, the refusal it answers (`None`: it stops):
/// access denied, interrupted, or another OS answer, each distinct.
type Tree = Vec<(i32, Option<io::ErrorKind>)>;

/// One seeded tree; the only failure is a draw the seeded stream refused.
fn tree(seed: u64) -> Result<Tree, DrawError> {
    let mut rng = Seeded::from_seed(seed);
    let count = rng.below(257)?;
    let mut targets = Vec::new();
    let mut pid: i32 = 100;
    for _ in 0..count {
        let refuses = rng.below(5)? == 0;
        let kind = if refuses {
            Some(match rng.below(3)? {
                0 => io::ErrorKind::PermissionDenied,
                1 => io::ErrorKind::Interrupted,
                _ => io::ErrorKind::Other,
            })
        } else {
            None
        };
        targets.push((pid, kind));
        pid = pid.saturating_add(1);
    }
    Ok(targets)
}

/// The issue's own case: the second of four targets refuses, and the third
/// and fourth are still signalled; the refusal is still the answer.
#[test]
fn a_refusal_mid_sweep_still_signals_every_later_target() -> Outcome {
    let mut signalled = Vec::new();
    let answer = signal_every([10, 11, 12, 13], |pid| {
        signalled.push(pid);
        if pid == 11 {
            return Err(io::Error::from(io::ErrorKind::PermissionDenied));
        }
        Ok(())
    });
    assert_eq!(signalled, vec![10, 11, 12, 13], "every target is signalled");
    assert_eq!(
        answer.map_err(|error| error.kind()),
        Err(io::ErrorKind::PermissionDenied),
        "the refusal is reported, not swallowed"
    );
    Ok(())
}

/// Every seeded tree is signalled whole, once per target, in order, and the
/// answer is the model's: the first refusal's kind, or `Ok`.
#[test]
fn sim_every_seeded_tree_is_signalled_whole_and_answers_the_first_refusal() -> Outcome {
    for seed in 0..SEEDS {
        let targets = tree(seed)?;
        let mut signalled = Vec::new();
        let answer = signal_every(targets.iter().map(|target| target.0), |pid| {
            signalled.push(pid);
            match targets
                .iter()
                .find(|target| target.0 == pid)
                .and_then(|target| target.1)
            {
                Some(kind) => Err(io::Error::from(kind)),
                None => Ok(()),
            }
        });
        let expected: Vec<i32> = targets.iter().map(|target| target.0).collect();
        assert_eq!(
            signalled, expected,
            "seed {seed}: each target once, in order"
        );
        let model = match targets.iter().find_map(|target| target.1) {
            Some(kind) => Err(kind),
            None => Ok(()),
        };
        assert_eq!(
            answer.map_err(|error| error.kind()),
            model,
            "seed {seed}: the first refusal is the answer"
        );
    }
    Ok(())
}

/// One seed draws one tree; distinct seeds draw distinct trees.
#[test]
fn sim_the_same_seed_replays_and_seeds_diverge() -> Outcome {
    let reference = tree(3)?;
    assert_eq!(tree(3)?, reference, "one seed, one tree");
    let mut distinct = 0_usize;
    for seed in 0..64 {
        if tree(seed)? != reference {
            distinct = distinct.saturating_add(1);
        }
    }
    assert!(distinct > 0, "seeds draw different trees");
    Ok(())
}
