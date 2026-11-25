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
