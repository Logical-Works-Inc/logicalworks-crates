#![cfg(test)]
//! Seeded sims for the temp scope's judges, each checked against a reference
//! model written from the rule, never from the code (INV-BOT-TEMP-1).
//!
//! Bugs each fool is for: fat-finger, a name one byte off `xcrun_db-XXXXXXXX`
//! is swept; clock-fool, a file stamped in the future, or one tick short of
//! idle, is swept; double-clicker, a second claim inside one interval sweeps;
//! saboteur, a root with no owner file, a file named like a root, or a root
//! whose owner is still locked is removed; wrong-order, children dropped in
//! any order leave the root behind, or remove it while one still lives.

use super::{
    ABANDONED_AFTER, OWNER, ROOT, ROOT_PREFIX, ROOTS_CLAIM, SWEEP_EVERY, Scratch, abandoned, claim,
    elapsed_enough, open_root, sweep_dead_roots, sweeps_roots,
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

/// The open root's path and live count, read under its lock.
fn open_root_state() -> Option<(std::path::PathBuf, usize)> {
    ROOT.hold()
        .as_ref()
        .map(|root| (root.path.clone(), root.live))
}

#[test]
fn sim_scratch_root_lives_exactly_while_a_child_does() -> SimResult {
    // The model: a count of live children. The root exists exactly while it is
    // above zero, every live child's directory exists under it, and a caller
    // that owns its temp gets nothing and moves nothing.
    for seed in 0..64_u64 {
        let mut draw = Seeded::from_seed(seed);
        let mut live: Vec<Scratch> = Vec::new();
        let mut roots_seen: Vec<std::path::PathBuf> = Vec::new();
        for _ in 0..=draw.below(24)? {
            match draw.below(4)? {
                0 | 1 => {
                    let scratch = Scratch::for_spawn(false)?.ok_or("a child gets a dir")?;
                    live.push(scratch);
                }
                2 if !live.is_empty() => {
                    // Any child, not the newest: the drawn one moves to the end
                    // and the truncate runs its drop.
                    let at = draw.index(live.len())?;
                    let last = live.len().saturating_sub(1);
                    live.swap(at, last);
                    live.truncate(last);
                }
                _ => {
                    assert!(
                        Scratch::for_spawn(true)?.is_none(),
                        "seed {seed}: the caller's own temp gets nothing"
                    );
                }
            }
            match open_root_state() {
                Some((root, count)) => {
                    assert_eq!(count, live.len(), "seed {seed}: the live count");
                    assert!(
                        root.join(OWNER).exists(),
                        "seed {seed}: an open root holds its owner"
                    );
                    for child in &live {
                        assert!(
                            child.dir.starts_with(&root) && child.dir.is_dir(),
                            "seed {seed}: a live child's dir is under the open root"
                        );
                    }
                    if !roots_seen.contains(&root) {
                        roots_seen.push(root);
                    }
                }
                None => assert!(live.is_empty(), "seed {seed}: no root while a child lives"),
            }
            for root in &roots_seen {
                let open = open_root_state().is_some_and(|(path, _)| &path == root);
                assert_eq!(
                    root.exists(),
                    open,
                    "seed {seed}: only the open root exists"
                );
            }
        }
        live.clear();
        assert!(
            open_root_state().is_none(),
            "seed {seed}: the last drop closes the root"
        );
        for root in &roots_seen {
            assert!(!root.exists(), "seed {seed}: no root outlives its children");
        }
    }
    Ok(())
}

/// #1130 meets the per-child root: a process reopens its root for each child,
/// so the dead-root sweep runs when this process is due and, for the shared
/// Darwin dir, only on the machine's claim. Model: a private parent sweeps
/// exactly when due and never gets a stamp, which would itself be residue.
#[test]
fn sim_scratch_root_sweep_claims_only_the_shared_dir() -> SimResult {
    let (parent, _owner) = open_root(&std::env::temp_dir())?;
    let base = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(1_700_000_000))
        .ok_or("the base instant overflows")?;
    for seed in 0..256_u64 {
        let mut draw = Seeded::from_seed(seed);
        let due = draw.below(2)? == 0;
        let now = base
            .checked_add(Duration::from_millis(draw.below(1_000_000)?))
            .ok_or("a drawn instant overflows")?;
        assert_eq!(
            sweeps_roots(&parent, due, now),
            due,
            "seed {seed}: a private parent sweeps exactly when due"
        );
        assert!(
            !parent.join(ROOTS_CLAIM).exists(),
            "seed {seed}: a private parent gets no stamp"
        );
    }
    fs::remove_dir_all(&parent)?;
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
        // The holder's lock lasts to the end of this iteration.
        let contended = last_win.is_some() && draw.below(6)? == 0;
        let _holder = if contended {
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
    // The sweep's instant: fixed, so the sim reads no clock. A dead owner file
    // is stamped far before it; a young one (an owner still opening its root)
    // one second before, inside the idle bound.
    let now = SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(1_700_000_000))
        .ok_or("the sweep instant overflows")?;
    let old = now
        .checked_sub(ABANDONED_AFTER.saturating_mul(6))
        .ok_or("the old instant underflows")?;
    let young = now
        .checked_sub(Duration::from_secs(1))
        .ok_or("the young instant underflows")?;
    for seed in 0..64_u64 {
        // 0: dead root, 1: live root, 2: root with no owner file, 3: a file
        // named like a root, 4: a directory whose name is not a root's, 5: a
        // root whose owner made it a moment ago and has not locked it yet.
        let kind = Seeded::from_seed(seed).below(6)?;
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
                owner.set_modified(if kind == 5 { young } else { old })?;
                if kind == 1 {
                    owner.lock()?;
                    held = Some(owner);
                }
            }
        }
        let removed = sweep_dead_roots(&parent, &keep, now);
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

#[test]
fn sim_scratch_a_root_removed_from_outside_is_replaced() -> SimResult {
    // The saboteur: a temp cleaner removes the open root while two children
    // live. The next child gets a fresh root, the two old drops count against
    // no root, and the fresh root lives exactly as long as its own child.
    let mut old = vec![
        Scratch::for_spawn(false)?.ok_or("a child gets a dir")?,
        Scratch::for_spawn(false)?.ok_or("a child gets a dir")?,
    ];
    let (gone, count) = open_root_state().ok_or("two children hold a root")?;
    assert_eq!(count, 2, "two children are counted");
    fs::remove_dir_all(&gone)?;
    let mut third = Some(Scratch::for_spawn(false)?.ok_or("a removed root is replaced")?);
    let dir = third
        .as_ref()
        .map(|child| child.dir.clone())
        .ok_or("the third child")?;
    let (fresh, count) = open_root_state().ok_or("the third child holds a root")?;
    assert!(
        fresh != gone && dir.starts_with(&fresh) && dir.is_dir(),
        "a fresh root"
    );
    assert_eq!(count, 1, "the fresh root counts only its own child");
    old.clear();
    assert_eq!(
        open_root_state(),
        Some((fresh.clone(), 1)),
        "old children's drops leave the fresh root open"
    );
    assert!(dir.is_dir(), "the live child's dir stays");
    third = None;
    assert!(
        third.is_none() && open_root_state().is_none(),
        "the last drop closes the root"
    );
    assert!(!fresh.exists(), "no root outlives its children");
    Ok(())
}
