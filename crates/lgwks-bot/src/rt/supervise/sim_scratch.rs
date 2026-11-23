#![cfg(test)]
//! Seeded sims for the temp scope's judges, each checked against a reference
//! model written from the rule, never from the code (INV-BOT-TEMP-1).
//!
//! Bugs each fool is for: fat-finger, a name one byte off `xcrun_db-XXXXXXXX`
//! is swept; clock-fool, a file stamped in the future, or one tick short of
//! idle, is swept; double-clicker, a second claim inside one interval sweeps;
//! saboteur, a root with no owner file, a file named like a root, or a root
//! whose owner is still locked is removed.

use super::{
    ABANDONED_AFTER, OWNER, ROOT_PREFIX, SWEEP_EVERY, abandoned, claim, elapsed_enough, open_root,
    sweep_dead_roots,
};
use lgwks_std::seeded::Seeded;
use std::error::Error;
use std::fs::{self, File};
use std::time::{Duration, SystemTime};

/// What a sim reports when a precondition did not hold.
type SimResult = Result<(), Box<dyn Error>>;

/// The model: `xcrun_db-` and exactly eight ASCII letters or digits.
fn shaped(name: &str) -> bool {
    let Some(rest) = name.strip_prefix("xcrun_db-") else {
        return false;
    };
    rest.len() == 8 && rest.chars().all(|letter| letter.is_ascii_alphanumeric())
}

/// Bytes a drawn suffix is built from: letters, digits and near misses.
const ALPHABET: [&str; 10] = ["a", "Z", "0", "9", "-", "_", ".", "é", "/", " "];
/// Prefixes a drawn name starts with: the real one and its neighbours.
const PREFIXES: [&str; 4] = ["xcrun_db-", "xcrun_db", "Xcrun_db-", "xcrun_db--"];

#[test]
fn sim_scratch_abandoned_matches_the_model() -> SimResult {
    let base = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(1_700_000_000))
        .ok_or("the base instant overflows")?;
    for seed in 0..4_000_u64 {
        let mut draw = Seeded::from_seed(seed);
        let len = draw.below(12)?;
        let mut name = String::from(*PREFIXES.get(draw.index(PREFIXES.len())?).ok_or("prefix")?);
        for _ in 0..len {
            name.push_str(ALPHABET.get(draw.index(ALPHABET.len())?).ok_or("letter")?);
        }
        let regular = draw.below(5)? != 0;
        // Ages around the bound, and some in the future.
        let age = draw.below(40)?;
        let ahead = draw.below(4)? == 0;
        let modified = if ahead {
            base.checked_add(Duration::from_secs(age))
        } else {
            base.checked_sub(Duration::from_secs(age))
        }
        .ok_or("a drawn instant overflows")?;
        let idle = !ahead && Duration::from_secs(age) >= ABANDONED_AFTER;
        let want = shaped(&name) && regular && idle;
        assert_eq!(
            abandoned(&name, regular, modified, base),
            want,
            "seed {seed}: {name:?} regular={regular} age={age} ahead={ahead}"
        );
    }
    Ok(())
}

#[test]
fn sim_scratch_sweep_interval_matches_the_model() -> SimResult {
    let every = u64::try_from(SWEEP_EVERY.as_millis())?;
    for seed in 0..4_000_u64 {
        let mut draw = Seeded::from_seed(seed);
        let last = draw.below(3)?.saturating_mul(draw.below(100_000)?);
        let at = last.saturating_add(draw.below(every.saturating_mul(2))?);
        let want = last == 0 || at.saturating_sub(last) >= every;
        assert_eq!(
            elapsed_enough(last, at),
            want,
            "seed {seed}: last={last} at={at}"
        );
    }
    assert!(
        !elapsed_enough(1, every),
        "one millisecond short is not due"
    );
    assert!(
        elapsed_enough(1, every.saturating_add(1)),
        "the full interval is due"
    );
    Ok(())
}

/// #1130: every process scanned the shared Darwin temp dir (30,145 entries on
/// the build Mac) at its first spawn and each interval, and logical_ci's lanes
/// ran seven times slower. The claim lets one process per machine scan it per
/// interval. Model: a claim wins when the stamp is new, or the last win is a
/// full interval behind, or more than one interval ahead (a stepped clock),
/// and never while another process holds the stamp's lock.
#[test]
fn sim_scratch_machine_claim_matches_the_model() -> SimResult {
    let (parent, _owner) = open_root(&std::env::temp_dir())?;
    let stamp = parent.join(".lgwks-sweep-sim");
    let base = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(1_700_000_000))
        .ok_or("the base instant overflows")?;
    let every = u64::try_from(SWEEP_EVERY.as_millis())?;
    let mut last_win: Option<SystemTime> = None;
    let mut now = base;
    for seed in 0..2_000_u64 {
        let mut draw = Seeded::from_seed(seed);
        // Steps around the interval, and some clock steps backwards.
        let step = Duration::from_millis(draw.below(every.saturating_mul(2))?);
        now = if draw.below(10)? == 0 {
            now.checked_sub(step.saturating_mul(3))
        } else {
            now.checked_add(step)
        }
        .ok_or("a drawn instant overflows")?;
        // A contended draw needs a stamp to lock, so the first win comes first.
        let contended = last_win.is_some() && draw.below(6)? == 0;
        let holder = if contended {
            let other = File::open(&stamp)?;
            other.lock()?;
            Some(other)
        } else {
            None
        };
        let due = match last_win {
            None => true,
            Some(last) => match now.duration_since(last) {
                Ok(age) => age >= SWEEP_EVERY,
                Err(ahead) => ahead.duration() > SWEEP_EVERY,
            },
        };
        let want = due && !contended;
        assert_eq!(
            claim(&stamp, now),
            want,
            "seed {seed}: due={due} contended={contended}"
        );
        if want {
            last_win = Some(now);
        }
        drop(holder);
        if last_win.is_none() {
            return Err("the first claim of a new stamp must win".into());
        }
    }
    fs::remove_dir_all(&parent)?;
    Ok(())
}

#[test]
fn sim_scratch_dead_roots_match_the_model() -> SimResult {
    // The parent is itself a held root, so no concurrent sweep removes it.
    let (parent, _parent_owner) = open_root(&std::env::temp_dir())?;
    let (keep, _mine) = open_root(&parent)?;
    assert!(
        keep.starts_with(&parent) && keep.join(OWNER).exists(),
        "a root holds its owner file"
    );
    for seed in 0..64_u64 {
        // 0: dead root, 1: live root, 2: root with no owner file, 3: a file
        // named like a root, 4: a directory whose name is not a root's.
        let kind = Seeded::from_seed(seed).below(5)?;
        let name = if kind == 4 {
            format!("other-{seed}")
        } else {
            format!("{ROOT_PREFIX}sim-{seed}")
        };
        let path = parent.join(&name);
        let mut held = None;
        if kind == 3 {
            fs::write(&path, b"x")?;
        } else {
            fs::create_dir_all(path.join("child"))?;
            if kind != 2 {
                let owner = File::create(path.join(OWNER))?;
                if kind == 1 {
                    owner.lock()?;
                    held = Some(owner);
                }
            }
        }
        let removed = sweep_dead_roots(&parent, &keep);
        let want_gone = kind == 0;
        assert_eq!(
            removed,
            usize::from(want_gone),
            "seed {seed} kind {kind}: removed count"
        );
        assert_eq!(!path.exists(), want_gone, "seed {seed} kind {kind}: {name}");
        assert!(keep.exists(), "seed {seed}: this process's own root stays");
        drop(held);
        if path.is_dir() {
            fs::remove_dir_all(&path)?;
        } else if path.exists() {
            fs::remove_file(&path)?;
        }
    }
    fs::remove_dir_all(&parent)?;
    Ok(())
}
