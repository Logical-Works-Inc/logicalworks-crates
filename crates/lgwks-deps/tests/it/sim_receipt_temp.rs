//! The shipped `ci-receipt` binary's write, against a neighbour that holds its
//! temporary names.
//!
//! Source: run 45522616196, where `cargo check -p lgwks_deps --target
//! wasm32-wasip1` failed because the receipt's temporary name came from
//! `lgwks_std::id`, which wasm does not build. The write now claims
//! `.<name>.<n>.tmp` by exclusive create. Each seed's neighbour holds a set of
//! those names, every one on some seeds, with bytes no receipt holds, and
//! sometimes an older receipt sits at the path. The oracle is the rule itself:
//! a write lands exactly when one name is free, never touches a held name,
//! leaves none of its own, and a refusal exits 2 with the path as it was.

use std::fs;
use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};

use crate::ci_receipt_cli::{logs_copy, parsed, receipt_run, said};
use crate::lock_fixture::Scratch;
use crate::replay::{Failure, refuse, replay_sweep};
use crate::sim::{Rng, Trace};

/// The temporary names one receipt path may take, as `ci-receipt` bounds them.
const TEMP_NAMES: u32 = 64;

/// Seeds of the sweep; each runs the binary twice.
const SEEDS: u64 = 24;

/// What a neighbour leaves in one held temporary: bytes no receipt holds.
const HELD: &str = "\u{0}another writer's \u{fffd} half receipt";

/// The text at `path`, or `None` when nothing is there.
fn text_at(path: &Path) -> Result<Option<String>, Failure> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

/// One neighbour world, judged; returns what the oracle saw.
fn neighbour_world(rng: &mut Rng) -> Result<String, Failure> {
    let scratch = Scratch::new("ci-receipt-temp")?;
    let logs = logs_copy(&scratch)?;
    let out = scratch.path().join("receipt.json");
    let all = rng.chance(250);
    let held: Vec<u32> = (0..TEMP_NAMES).filter(|_| all || rng.chance(400)).collect();
    for number in &held {
        fs::write(
            scratch.path().join(format!(".receipt.json.{number}.tmp")),
            HELD,
        )?;
    }
    let old = rng.chance(500).then(|| "{\"old\":true}".to_owned());
    if let Some(text) = old.as_deref() {
        fs::write(&out, text)?;
    }
    let output = receipt_run(&logs, &out, &[])?;
    let full = held.len() == usize::try_from(TEMP_NAMES)?;
    match (full, output.status.code()) {
        (true, Some(2)) if text_at(&out)? == old => {}
        (false, Some(code)) if code != 2 && parsed(&out).is_ok() => {}
        (_, code) => {
            return refuse(format!(
                "{} of {TEMP_NAMES} held, exit {code:?}: {}",
                held.len(),
                said(&output)
            ));
        }
    }
    for number in 0..TEMP_NAMES {
        let temp = scratch.path().join(format!(".receipt.json.{number}.tmp"));
        let mine = held.contains(&number);
        if mine && text_at(&temp)?.as_deref() != Some(HELD) {
            return refuse(format!("the held temporary {number} was touched"));
        }
        if !mine && temp.exists() {
            return refuse(format!("the write left temporary {number} behind"));
        }
    }
    Ok(format!("{} held, full {full}", held.len()))
}

#[test]
fn sim_a_receipt_write_takes_a_temporary_no_writer_holds() -> Result<(), Failure> {
    replay_sweep("receipt-temp", SEEDS, |rng: &mut Rng, trace: &mut Trace| {
        let seen = neighbour_world(rng)?;
        trace.record(&seen);
        Ok(())
    })
}

/// Seeds of each hostile-path sweep below.
const PATH_SEEDS: u64 = 12;

/// The temporaries beside `dir/<name>` that exist, by number.
fn temps_of(dir: &Path, name: &str) -> Vec<u32> {
    (0..TEMP_NAMES)
        .filter(|number| dir.join(format!(".{name}.{number}.tmp")).exists())
        .collect()
}

/// Refuses unless `output` is a written receipt at `out`, not a refusal.
fn landed(output: &Output, out: &Path) -> Result<(), Failure> {
    if output.status.code().is_some_and(|code| code != 2) && parsed(out).is_ok() {
        return Ok(());
    }
    refuse(format!("no receipt landed: {}", said(output)))
}

/// Refuses unless `output` is a refusal, exit 2 with a `ci-receipt:` line.
fn refused(output: &Output) -> Result<(), Failure> {
    if output.status.code() == Some(2) && said(output).starts_with("ci-receipt: ") {
        return Ok(());
    }
    refuse(format!(
        "not refused: {:?} {}",
        output.status.code(),
        said(output)
    ))
}

/// Double-clickers: 2 to 8 writers of one path start at once. The model is a
/// solo run into the same directory: the path ends as those bytes exactly,
/// whichever writer renamed last, every writer lands, and no temporary stays.
#[test]
fn sim_racing_writers_leave_one_whole_receipt() -> Result<(), Failure> {
    replay_sweep(
        "receipt-race",
        PATH_SEEDS,
        |rng: &mut Rng, trace: &mut Trace| {
            let scratch = Scratch::new("ci-receipt-race")?;
            let logs = logs_copy(&scratch)?;
            let solo = scratch.path().join("solo.json");
            landed(&receipt_run(&logs, &solo, &[])?, &solo)?;
            let out = scratch.path().join("receipt.json");
            let writers = rng.between(2, 8);
            let mut children = Vec::new();
            for _ in 0..writers {
                children.push(
                    Command::new(env!("CARGO_BIN_EXE_ci-receipt"))
                        .args([Path::new("--logs"), &logs, Path::new("--out"), &out])
                        .stderr(Stdio::piped())
                        .spawn()?,
                );
            }
            for child in children {
                landed(&child.wait_with_output()?, &out)?;
            }
            if fs::read(&out)? != fs::read(&solo)? {
                return refuse(format!(
                    "{writers} racing writers left bytes no writer wrote"
                ));
            }
            if !temps_of(scratch.path(), "receipt.json").is_empty() {
                return refuse("a racing writer left its temporary");
            }
            trace.record_number("writers", writers);
            Ok(())
        },
    )
}

/// Saboteur: the held names are directories, some with files in them. A
/// directory is a held name like a file: skipped, never entered or removed.
#[test]
fn sim_a_held_name_that_is_a_directory_is_skipped() -> Result<(), Failure> {
    replay_sweep(
        "receipt-dirs",
        PATH_SEEDS,
        |rng: &mut Rng, trace: &mut Trace| {
            let scratch = Scratch::new("ci-receipt-dirs")?;
            let logs = logs_copy(&scratch)?;
            let out = scratch.path().join("receipt.json");
            let held: Vec<u32> = (0..rng.between(1, 63)).collect();
            for number in &held {
                let dir = scratch.path().join(format!(".receipt.json.{number}.tmp"));
                fs::create_dir(&dir)?;
                if rng.coin() {
                    fs::write(dir.join("inside"), HELD)?;
                }
            }
            landed(&receipt_run(&logs, &out, &[])?, &out)?;
            for number in &held {
                if !scratch
                    .path()
                    .join(format!(".receipt.json.{number}.tmp"))
                    .is_dir()
                {
                    return refuse(format!("held directory {number} was removed"));
                }
            }
            if temps_of(scratch.path(), "receipt.json").len() != held.len() {
                return refuse("the write left a temporary of its own");
            }
            trace.record_number("held", held.len());
            Ok(())
        },
    )
}

/// Fat-finger: the output's parent, at a seeded depth, is a regular file. The
/// write is refused with exit 2, and the file is left as it was.
#[test]
fn sim_an_out_path_under_a_file_is_refused() -> Result<(), Failure> {
    replay_sweep(
        "receipt-under-file",
        PATH_SEEDS,
        |rng: &mut Rng, trace: &mut Trace| {
            let scratch = Scratch::new("ci-receipt-under-file")?;
            let logs = logs_copy(&scratch)?;
            let blocker = scratch.path().join("blocker");
            fs::write(&blocker, HELD)?;
            let depth = rng.between(0, 3);
            let mut out = blocker.clone();
            for level in 0..depth {
                out = out.join(format!("level{level}"));
            }
            let out = out.join("receipt.json");
            refused(&receipt_run(&logs, &out, &[])?)?;
            if text_at(&blocker)?.as_deref() != Some(HELD) {
                return refuse("the file in the way was changed");
            }
            trace.record_number("depth", depth);
            Ok(())
        },
    )
}

/// Wrong-order: the output path is an existing directory with seeded entries.
/// The rename onto it is refused with exit 2, the directory keeps every entry,
/// and the temporary is removed.
#[test]
fn sim_an_out_path_that_is_a_directory_is_refused() -> Result<(), Failure> {
    replay_sweep(
        "receipt-out-dir",
        PATH_SEEDS,
        |rng: &mut Rng, trace: &mut Trace| {
            let scratch = Scratch::new("ci-receipt-out-dir")?;
            let logs = logs_copy(&scratch)?;
            let out = scratch.path().join("receipt.json");
            fs::create_dir(&out)?;
            let entries = rng.between(0, 4);
            for entry in 0..entries {
                fs::write(out.join(format!("entry{entry}")), HELD)?;
            }
            refused(&receipt_run(&logs, &out, &[])?)?;
            if !out.is_dir() || fs::read_dir(&out)?.count() != usize::try_from(entries)? {
                return refuse("the directory in the way lost an entry");
            }
            if !temps_of(scratch.path(), "receipt.json").is_empty() {
                return refuse("a refused rename left its temporary");
            }
            trace.record_number("entries", entries);
            Ok(())
        },
    )
}

/// Neighbour on both outputs: the JSON receipt and its Markdown page each
/// have their own names held. Each claims a free one of its own, both land,
/// and neither touches the other's held names.
#[test]
fn sim_both_outputs_claim_their_own_temporaries() -> Result<(), Failure> {
    replay_sweep(
        "receipt-both",
        PATH_SEEDS,
        |rng: &mut Rng, trace: &mut Trace| {
            let scratch = Scratch::new("ci-receipt-both")?;
            let logs = logs_copy(&scratch)?;
            let out = scratch.path().join("receipt.json");
            let page = scratch.path().join("receipt.md");
            let mut held = 0_usize;
            for name in ["receipt.json", "receipt.md"] {
                for number in 0..TEMP_NAMES - 1 {
                    if rng.chance(500) {
                        fs::write(scratch.path().join(format!(".{name}.{number}.tmp")), HELD)?;
                        held += 1;
                    }
                }
            }
            landed(
                &receipt_run(&logs, &out, &[Path::new("--markdown"), &page])?,
                &out,
            )?;
            if text_at(&page)?.is_none_or(|text| text.is_empty()) {
                return refuse("the Markdown page was not written");
            }
            let left = temps_of(scratch.path(), "receipt.json").len()
                + temps_of(scratch.path(), "receipt.md").len();
            if left != held {
                return refuse(format!("{held} held, {left} temporaries after the write"));
            }
            trace.record_number("held", held);
            Ok(())
        },
    )
}

/// Giant crowd: every name of the JSON receipt is held but one, the last.
/// The write takes that one, so the bound is reached and not passed.
#[test]
fn sim_the_last_free_name_is_taken() -> Result<(), Failure> {
    replay_sweep(
        "receipt-last",
        PATH_SEEDS,
        |rng: &mut Rng, trace: &mut Trace| {
            let scratch = Scratch::new("ci-receipt-last")?;
            let logs = logs_copy(&scratch)?;
            let out = scratch.path().join("receipt.json");
            let free = rng.below(TEMP_NAMES);
            for number in (0..TEMP_NAMES).filter(|number| *number != free) {
                fs::write(
                    scratch.path().join(format!(".receipt.json.{number}.tmp")),
                    HELD,
                )?;
            }
            landed(&receipt_run(&logs, &out, &[])?, &out)?;
            let left = temps_of(scratch.path(), "receipt.json");
            if left.contains(&free) || left.len() != usize::try_from(TEMP_NAMES - 1)? {
                return refuse(format!(
                    "free name {free}: {} temporaries after",
                    left.len()
                ));
            }
            trace.record_number("free", free);
            Ok(())
        },
    )
}
