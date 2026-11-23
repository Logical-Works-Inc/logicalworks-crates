//! Seeded sims of the residue census against a model built from the generator,
//! never from the census's own answer (ADR 0078, #1128 G2).
//!
//! Source: 2026-11-23, the build Mac's 110,965 leaked temp files that no test
//! saw. Fools and the bug each is for: weird-bytes, a name the walk skips or
//! miscounts; monkey, a deep tree the walk does not descend; saboteur, a
//! symlink back to the root that a followed walk loops on, and a directory the
//! census cannot read, which must fail closed rather than read as clean.

use super::{Census, Residue, count_within};
use lgwks_std::seeded::Seeded;
use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// What a sim reports when a precondition did not hold.
type SimResult = Result<(), Box<dyn Error>>;

/// Name parts a drawn entry is built from: plain, near misses and weird bytes.
const PARTS: [&str; 8] = [
    "a",
    "Z9",
    ".hidden",
    "with space",
    "é",
    "-dash",
    "x.tmp",
    "lwc-test-",
];

/// A fresh base dir of this test's own under the temp dir.
fn base(tag: &str) -> io::Result<PathBuf> {
    let unique = u64::from_be_bytes(lgwks_std::random::bytes::<8>().map_err(io::Error::other)?);
    let dir = std::env::temp_dir().join(format!("sim-residue-{tag}-{unique:016x}"));
    fs::create_dir(&dir)?;
    Ok(dir)
}

/// A drawn element of `items`, as an I/O error when the stream refuses.
fn pick<'a, T>(draw: &mut Seeded, items: &'a [T]) -> io::Result<&'a T> {
    let at = draw.index(items.len()).map_err(io::Error::other)?;
    items
        .get(at)
        .ok_or_else(|| io::Error::other("a drawn index is past the list"))
}

/// Build a drawn tree under `root`; returns every entry made, relative.
fn draw_tree(draw: &mut Seeded, root: &Path) -> io::Result<BTreeSet<PathBuf>> {
    let mut made = BTreeSet::new();
    let mut dirs = vec![PathBuf::new()];
    let steps = draw.below(14).map_err(io::Error::other)?;
    for step in 0..steps {
        let parent = pick(draw, &dirs)?.clone();
        let part = pick(draw, &PARTS)?;
        let name = parent.join(format!("{part}{step}"));
        match draw.below(4).map_err(io::Error::other)? {
            0 => {
                fs::create_dir(root.join(&name))?;
                dirs.push(name.clone());
            }
            1 => std::os::unix::fs::symlink(root, root.join(&name))?,
            _ => fs::write(root.join(&name), b"left")?,
        }
        made.insert(name);
    }
    Ok(made)
}

#[test]
fn sim_residue_census_matches_the_model() -> SimResult {
    let base = base("model")?;
    for seed in 0..256_u64 {
        let mut draw = Seeded::from_seed(seed);
        let census = Census::open(&base)?;
        let dir = census.dir().to_path_buf();
        assert!(
            dir.starts_with(&base) && dir.is_dir(),
            "seed {seed}: a fresh dir under the base"
        );
        let made = draw_tree(&mut draw, &dir)?;
        let mut residue = Residue::default();
        census.close(&mut residue);
        let named: BTreeSet<PathBuf> = made.iter().take(8).cloned().collect();
        assert_eq!(
            residue.total,
            made.len(),
            "seed {seed}: every entry is counted once"
        );
        assert_eq!(
            residue.named, named,
            "seed {seed}: the first eight, sorted, are named"
        );
        assert!(!residue.capped, "seed {seed}");
        assert!(
            residue.refusals.is_empty(),
            "seed {seed}: {:?}",
            residue.refusals
        );
        assert_eq!(
            residue.is_clean(),
            made.is_empty(),
            "seed {seed}: clean exactly when empty"
        );
        assert!(!dir.exists(), "seed {seed}: the census removes its dir");
        let verdict = residue.to_string();
        if !made.is_empty() {
            assert!(
                verdict.contains(&format!("left {} entries", made.len())),
                "seed {seed}: {verdict}"
            );
        }
    }
    assert_eq!(
        fs::read_dir(&base)?.count(),
        0,
        "the census leaves the base empty"
    );
    fs::remove_dir(&base)?;
    Ok(())
}

#[test]
fn sim_residue_an_unreadable_dir_is_never_clean() -> SimResult {
    use std::os::unix::fs::PermissionsExt as _;
    let base = base("unreadable")?;
    let census = Census::open(&base)?;
    let locked = census.dir().join("locked");
    fs::create_dir(&locked)?;
    fs::write(locked.join("inside"), b"x")?;
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000))?;
    // Root reads a mode-000 directory anyway; then the census counts into it.
    let readable = fs::read_dir(&locked).is_ok();
    let dir = census.dir().to_path_buf();
    let mut residue = Residue::default();
    census.close(&mut residue);
    if dir.exists() {
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700))?;
        fs::remove_dir_all(&dir)?;
    }
    fs::remove_dir(&base)?;
    assert!(!residue.is_clean(), "a dir left behind is never clean");
    if readable {
        assert_eq!(residue.total, 2, "a readable dir is counted into");
    } else {
        assert!(
            !residue.refusals.is_empty(),
            "an unreadable dir is a refusal, so the census fails closed"
        );
        assert!(residue.to_string().contains("could not judge"), "{residue}");
    }
    Ok(())
}

/// Giant: a tree past the walk's bound stops at the bound and says it is a
/// floor, `3+`, never a smaller exact count.
#[test]
fn sim_residue_a_tree_past_the_bound_is_capped_and_says_so() -> SimResult {
    let base = base("capped")?;
    for name in ["a", "b", "c", "d", "e"] {
        fs::write(base.join(name), b"x")?;
    }
    let mut residue = Residue::default();
    count_within(&base, &mut residue, 3);
    fs::remove_dir_all(&base)?;
    assert_eq!(residue.total, 3, "the walk stops at the bound");
    assert!(residue.capped, "a stopped walk is capped");
    assert!(residue.to_string().contains("left 3+ entries"), "{residue}");
    Ok(())
}

/// Saboteur: unreadable dirs between readable ones. Each one is a refusal, and
/// the walk goes on, so every readable file is still counted.
#[test]
fn sim_residue_an_unreadable_dir_does_not_hide_its_siblings() -> SimResult {
    use std::os::unix::fs::PermissionsExt as _;
    let base = base("siblings")?;
    let mut locked = Vec::new();
    for index in 0..8 {
        let dir = base.join(format!("d{index}"));
        fs::create_dir(&dir)?;
        fs::write(dir.join("f"), b"x")?;
        if index % 2 == 0 {
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o000))?;
            locked.push(dir);
        }
    }
    let readable = fs::read_dir(locked.first().ok_or("a dir was locked")?).is_ok();
    let mut residue = Residue::default();
    count_within(&base, &mut residue, usize::MAX);
    for dir in &locked {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    fs::remove_dir_all(&base)?;
    if readable {
        // Root reads a mode-000 dir: 8 dirs and 8 files, nothing refused.
        assert_eq!(residue.total, 16, "{residue}");
        return Ok(());
    }
    assert_eq!(residue.refusals.len(), 4, "each locked dir is refused");
    assert_eq!(
        residue.total, 12,
        "8 dirs and the 4 readable files: {residue}"
    );
    Ok(())
}

/// The verdict is files and refusals, every combination against the model:
/// clean exactly when nothing was left and nothing refused, and each part
/// named exactly when it is there. A process census is not claimed (G2b).
#[test]
fn sim_residue_the_verdict_matches_the_model_for_every_shape() {
    for total in [0_usize, 1, 9] {
        for capped in [false, true] {
            for refused in [false, true] {
                let named: BTreeSet<PathBuf> = (0..total.min(8))
                    .map(|index| PathBuf::from(format!("e{index}")))
                    .collect();
                let mut residue = Residue {
                    named,
                    total,
                    capped,
                    ..Residue::default()
                };
                if refused {
                    residue.refusals.push("no".to_owned());
                }
                // The record holds only what the census fills: no process
                // field that no step of it sets (G2b).
                let record = format!("{residue:?}");
                assert!(!record.contains("processes"), "{record}");
                let verdict = residue.to_string();
                let shape = format!("total {total}, capped {capped}, refused {refused}");
                assert_eq!(residue.is_clean(), total == 0 && !refused, "{shape}");
                assert_eq!(
                    verdict.contains("entries in its TMPDIR"),
                    total > 0,
                    "{shape}"
                );
                assert_eq!(verdict.contains("could not judge: no"), refused, "{shape}");
                let floor = format!("left {total}+ entries");
                assert_eq!(verdict.contains(&floor), total > 0 && capped, "{shape}");
                assert!(!verdict.contains("process"), "{shape}: {verdict}");
            }
        }
    }
}

/// A census that only refused names no entries: its verdict is the refusal.
#[test]
fn sim_residue_a_refusal_alone_names_no_entries() {
    let mut residue = Residue::default();
    residue.refuse("the host said no".to_owned());
    let verdict = residue.to_string();
    assert!(!residue.is_clean(), "a refusal is never clean");
    assert!(
        verdict.contains("could not judge: the host said no"),
        "{verdict}"
    );
    assert!(!verdict.contains("entries in its TMPDIR"), "{verdict}");
    assert!(!verdict.contains("live processes"), "{verdict}");
}
