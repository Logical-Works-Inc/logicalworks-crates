//! Seeded fools against the split requirement spine (ADR 0023). The reference model
//! is the same requirements written in one file: however a seed scatters them over
//! pages, the lock the check writes must be the lock the one file gives. The
//! fat-finger copies an id, the saboteur edits behind a copy, the wrong-order fool
//! names pages against the id order, and weird-bytes leaves a page that is not text.

use std::collections::BTreeSet;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

use lgwks_deps::requirements::execute;

use crate::seed::{Rng, Trace};

/// A test's result type.
type TestResult = Result<(), Box<dyn Error>>;

/// How many spines each simulation scatters; each writes a small tree to disk.
const SEEDS: u64 = 128;

/// A fresh, empty tree. Its name is the one draw no seed drives: it is never in a
/// trace and only keeps two runs of the suite apart.
fn tree() -> Result<PathBuf, Box<dyn Error>> {
    let nonce = u64::from_le_bytes(lgwks_std::random::bytes::<8>()?);
    let root = std::env::temp_dir().join(format!("lwc-sim-requirements-{nonce:016x}"));
    fs::create_dir_all(root.join("docs/requirements"))?;
    fs::create_dir_all(root.join("scripts"))?;
    Ok(root)
}

/// A check of `root`, with the exit code or the refusal's text.
fn check(root: &Path, update: bool) -> Result<u8, String> {
    let mut out = Vec::new();
    let mut err = Vec::new();
    match execute(root, update, &mut out, &mut err) {
        Ok(code) => Ok(code),
        Err(error) => Err(error.to_string()),
    }
}

/// One requirement as the page holds it.
fn section(id: u32, seed: u64) -> String {
    format!("### R{id}. Title\n\n> Sentence {id} of spine {seed}.\n\n")
}

/// `count` requirements drawn for `seed`, in id order.
fn drawn_ids(rng: &mut Rng) -> Vec<u32> {
    (1..=rng.between(1, 24)).collect()
}

/// The ids scattered over the spine and `pages` continuation pages by `rng`, the
/// pages named so their name order disagrees with the order of the ids.
fn scatter(root: &Path, ids: &[u32], pages: u32, seed: u64, rng: &mut Rng) -> TestResult {
    let mut bodies = vec![String::new(); usize::try_from(pages)?.saturating_add(1)];
    for id in ids {
        let at = usize::try_from(rng.below(pages.saturating_add(1)))?;
        if let Some(body) = bodies.get_mut(at) {
            body.push_str(&section(*id, seed));
        }
    }
    let (first, rest) = bodies.split_first().ok_or("no spine")?;
    fs::write(root.join("REQUIREMENTS.md"), format!("# R\n\n{first}"))?;
    for (index, body) in rest.iter().enumerate() {
        let name = rest.len().saturating_sub(index);
        fs::write(root.join(format!("docs/requirements/{name:03}.md")), body)?;
    }
    Ok(())
}

/// The lock's rows, without its comment header.
fn rows(root: &Path) -> Result<Vec<String>, Box<dyn Error>> {
    let lock = fs::read_to_string(root.join("scripts/requirements.lock"))?;
    Ok(lock
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(str::to_owned)
        .collect())
}

/// One seed's spine, both ways; the trace records what the lock held.
fn scattered_against_whole(seed: u64) -> Result<Trace, Box<dyn Error>> {
    let mut rng = Rng::new(seed);
    let ids = drawn_ids(&mut rng);
    let pages = rng.below(8);
    let split = tree()?;
    scatter(&split, &ids, pages, seed, &mut rng)?;
    let whole = tree()?;
    let one: String = ids.iter().map(|id| section(*id, seed)).collect();
    fs::write(whole.join("REQUIREMENTS.md"), format!("# R\n\n{one}"))?;
    assert_eq!(check(&split, true), Ok(0), "seed {seed}: the split update");
    assert_eq!(check(&whole, true), Ok(0), "seed {seed}: the whole update");
    let held = rows(&split)?;
    assert_eq!(held, rows(&whole)?, "seed {seed}: two spines, two locks");
    assert_eq!(
        check(&split, false),
        Ok(0),
        "seed {seed}: the lock verifies"
    );
    let mut trace = Trace::new();
    trace.record_number("ids", ids.len());
    trace.record_number("pages", usize::try_from(pages)?);
    for row in &held {
        trace.record_number("row", row.len());
    }
    fs::remove_dir_all(&split)?;
    fs::remove_dir_all(&whole)?;
    Ok(trace)
}

/// Catches a reader that drops a page, reads a folder out of order into the lock,
/// or answers differently for a spine split than for the same spine in one file.
#[test]
fn sim_requirements_pages_wrong_order_a_scatter_locks_as_the_one_file_does() -> TestResult {
    for seed in 0..SEEDS {
        scattered_against_whole(seed)?;
    }
    Ok(())
}

/// The saboteur: edit a requirement on an early page and leave a copy of the old
/// sentence on a later one. A reader whose later copy wins would report no change.
/// Catches a parse that lets a duplicate id overwrite the first declaration.
#[test]
fn sim_requirements_pages_saboteur_an_edit_behind_a_copy_is_refused() -> TestResult {
    for seed in 0..SEEDS {
        let mut rng = Rng::new(seed);
        let ids = drawn_ids(&mut rng);
        let root = tree()?;
        scatter(&root, &ids, rng.below(6), seed, &mut rng)?;
        assert_eq!(check(&root, true), Ok(0), "seed {seed}: the update");
        let locked = rows(&root)?;
        let index = usize::try_from(rng.below(u32::try_from(ids.len())?))?;
        let id = *ids.get(index).ok_or("no drawn id")?;
        fs::write(root.join("docs/requirements/zzz.md"), section(id, seed))?;
        let verdict = check(&root, false);
        let refused = verdict
            .as_ref()
            .err()
            .is_some_and(|message| message.contains(&format!("R{id} is declared twice")));
        assert!(refused, "seed {seed}: a copy of R{id} gave {verdict:?}");
        assert_eq!(
            rows(&root)?,
            locked,
            "seed {seed}: a refusal moved the lock"
        );
        fs::remove_dir_all(&root)?;
    }
    Ok(())
}

/// The neighbour and weird-bytes: a sibling tree shares no state, a text file in
/// the folder is not a page, and a page that is not UTF-8 is refused by name
/// instead of read as empty. Catches a reader that swallows an unreadable page.
#[test]
fn sim_requirements_pages_weird_bytes_a_bad_page_is_named_and_noise_is_ignored() -> TestResult {
    let mut rng = Rng::new(7);
    for seed in 0..SEEDS {
        let ids = drawn_ids(&mut rng);
        let root = tree()?;
        let neighbour = tree()?;
        scatter(&root, &ids, 3, seed, &mut rng)?;
        scatter(&neighbour, &ids, 1, seed.saturating_add(1), &mut rng)?;
        fs::write(root.join("docs/requirements/notes.txt"), [0xFF, 0x00, 0xFE])?;
        assert_eq!(
            check(&root, true),
            Ok(0),
            "seed {seed}: noise is not a page"
        );
        assert_eq!(check(&neighbour, true), Ok(0), "seed {seed}: the neighbour");
        assert_ne!(rows(&root)?, rows(&neighbour)?, "seed {seed}: trees mixed");
        fs::write(root.join("docs/requirements/bad.md"), [0xFF, 0x00, 0xFE])?;
        let verdict = check(&root, false);
        let named = verdict
            .as_ref()
            .err()
            .is_some_and(|message| message.contains("bad.md"));
        assert!(named, "seed {seed}: a page not in UTF-8 gave {verdict:?}");
        fs::remove_dir_all(&root)?;
        fs::remove_dir_all(&neighbour)?;
    }
    Ok(())
}

/// One seed gives one trace and two seeds differ, so a failure replays exactly.
#[test]
fn sim_requirements_pages_monkey_the_same_seed_replays_and_seeds_diverge() -> TestResult {
    let mut hashes = BTreeSet::new();
    for seed in 0..SEEDS {
        let hash = scattered_against_whole(seed)?.hash();
        assert_eq!(
            hash,
            scattered_against_whole(seed)?.hash(),
            "seed {seed} replayed differently"
        );
        hashes.insert(hash);
    }
    assert!(
        hashes.len() > 8,
        "seeds collapsed to {} traces",
        hashes.len()
    );
    Ok(())
}
