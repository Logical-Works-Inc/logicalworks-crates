//! Seeded simulation of where a continuation lives and when it is due (#1016).
//!
//! #1016 split `journal::continuation` into the checkpoint record, `naming` (where a successor
//! lives) and `watermark` (when a journal continues), and made both crate-private: no caller outside
//! the crate names them, and the file journal is their one user. These families drive the shipped
//! functions with what a careless or hostile caller hands the journal: names full of dots, digits
//! and non-ASCII, bases that look like generations, paths with no file name, and trigger points on
//! either side of each ceiling. One seed draws every input; each family runs every seed twice and
//! requires the same trace hash, so a run that depended on anything but its seed fails.

use std::num::TryFromIntError;
use std::path::{Path, PathBuf};

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::seeded::{DrawError, Seeded};

use super::naming::GENERATION_DIGITS;
use super::watermark::{SHIPPED_BYTE_CEILING, SHIPPED_EVENT_CEILING};
use super::{
    ContinuationPolicy, ContinuationWatermark, generation_of, is_generation_path, successor_path,
};

/// Seeds each family sweeps.
const SEEDS: u64 = 256;

/// The characters a hostile file name is drawn from: dots, dashes, spaces, digits and non-ASCII.
const ALPHABET: [char; 12] = ['j', 'r', '.', '-', '_', ' ', '0', '7', '9', 'é', 'ß', '中'];

/// Paths that carry no file name: empty, the root, and parent references.
const NAMELESS: [&str; 5] = ["", "/", "..", "a/..", "/.."];

/// How a family broke.
#[derive(Debug)]
enum SimFailed {
    /// The seeded stream refused a draw.
    Draw(DrawError),
    /// A draw did not fit the index it was drawn for.
    Index(TryFromIntError),
    /// A property did not hold for `seed`.
    Broke {
        /// The seed the inputs were drawn from.
        seed: u64,
        /// Which property broke.
        property: &'static str,
    },
}

impl std::fmt::Display for SimFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Draw(cause) => write!(f, "draw: {cause}"),
            Self::Index(cause) => write!(f, "index: {cause}"),
            Self::Broke { seed, property } => write!(f, "seed {seed}: {property}"),
        }
    }
}

/// One family: draw from the stream, assert, and hash what it observed.
type Family = fn(&mut Seeded, &mut Hasher, u64) -> Result<(), SimFailed>;

/// One seeded draw below `bound`.
fn draw(stream: &mut Seeded, bound: u64) -> Result<u64, SimFailed> {
    stream.below(bound).map_err(SimFailed::Draw)
}

/// One seeded index below `bound`.
fn index(stream: &mut Seeded, bound: u64) -> Result<usize, SimFailed> {
    usize::try_from(draw(stream, bound)?).map_err(SimFailed::Index)
}

/// `Ok` when `holds`, otherwise a logged refusal naming `property`.
fn hold(seed: u64, holds: bool, property: &'static str) -> Result<(), SimFailed> {
    if holds {
        return Ok(());
    }
    let failure = SimFailed::Broke { seed, property };
    lgwks_std::trace::warn!(%failure, "sim_naming: a property broke");
    Err(failure)
}

/// A file name that never parses as a generation: a letter, then up to 39 hostile characters.
fn hostile_name(stream: &mut Seeded, seed: u64) -> Result<String, SimFailed> {
    let mut name = String::from("j");
    for _ in 0..draw(stream, 40)? {
        let Some(glyph) = ALPHABET.get(index(stream, 12)?) else {
            return hold(seed, false, "a glyph draw falls inside the alphabet").map(|()| name);
        };
        name.push(*glyph);
    }
    Ok(name)
}

/// The tenant directory a journal lives in: plain, never a `.cont` directory.
fn tenant_dir(stream: &mut Seeded) -> Result<PathBuf, SimFailed> {
    Ok(PathBuf::from(format!("tenant-{}", draw(stream, 1_000)?)))
}

/// The trace hash of `family` over `seed`.
fn trace(family: Family, seed: u64) -> Result<Digest, SimFailed> {
    let mut stream = Seeded::from_seed(seed);
    let mut hasher = Hasher::new();
    family(&mut stream, &mut hasher, seed)?;
    Ok(hasher.finalize())
}

/// Run `family` on every seed twice and require the same trace hash each time.
fn replays(family: Family) -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        let first = trace(family, seed)?;
        hold(
            seed,
            first == trace(family, seed)?,
            "the trace hash did not replay",
        )?;
    }
    Ok(())
}

/// Any base name continues into `<base>.cont/000001`, which is generation two.
fn base_names(stream: &mut Seeded, hasher: &mut Hasher, seed: u64) -> Result<(), SimFailed> {
    let base = tenant_dir(stream)?.join(hostile_name(stream, seed)?);
    let next = successor_path(&base);
    let expected = PathBuf::from(format!("{}.cont", base.display())).join("000001");
    hold(
        seed,
        next == expected,
        "a base continues into its .cont/000001",
    )?;
    hold(
        seed,
        !is_generation_path(&base),
        "a base is not a generation",
    )?;
    hold(
        seed,
        is_generation_path(&next),
        "its successor is this scheme's own",
    )?;
    hold(seed, generation_of(&base) == 1, "a base is generation one")?;
    hold(
        seed,
        generation_of(&next) == 2,
        "its first successor is generation two",
    )?;
    hasher.update(next.to_string_lossy().as_bytes());
    Ok(())
}

/// Walking a chain counts one generation per step at one name width in one directory.
fn chains(stream: &mut Seeded, hasher: &mut Hasher, seed: u64) -> Result<(), SimFailed> {
    let steps = draw(stream, 60)?.saturating_add(1);
    let mut path = successor_path(&tenant_dir(stream)?.join(hostile_name(stream, seed)?));
    let directory = path.parent().map(Path::to_path_buf);
    for step in 0..steps {
        hold(
            seed,
            generation_of(&path) == step.saturating_add(2),
            "one generation per step",
        )?;
        let width = path.file_name().map(|name| name.len());
        hold(
            seed,
            width == Some(GENERATION_DIGITS),
            "generation names keep one width",
        )?;
        hold(
            seed,
            path.parent().map(Path::to_path_buf) == directory,
            "one directory",
        )?;
        path = successor_path(&path);
    }
    hasher.update(&steps.to_be_bytes());
    hasher.update(path.to_string_lossy().as_bytes());
    Ok(())
}

/// A six-digit base is a generation only inside a `.cont` directory.
fn six_digit_bases(stream: &mut Seeded, hasher: &mut Hasher, seed: u64) -> Result<(), SimFailed> {
    let number = draw(stream, 1_000_000)?;
    let digits = format!("{number:06}");
    let base = tenant_dir(stream)?.join(&digits);
    hold(
        seed,
        !is_generation_path(&base),
        "six digits outside .cont are a base",
    )?;
    hold(
        seed,
        generation_of(&base) == 1,
        "an ambiguous base still counts as one",
    )?;
    let inside = tenant_dir(stream)?.join("bot.jrnl.cont").join(&digits);
    hold(
        seed,
        is_generation_path(&inside),
        "six digits inside .cont are a generation",
    )?;
    let counted = generation_of(&inside) == number.saturating_add(1);
    hold(seed, counted, "a generation counts its name plus one")?;
    hasher.update(digits.as_bytes());
    Ok(())
}

/// A path with no file name names no journal, so it continues into itself at generation one.
fn nameless_paths(stream: &mut Seeded, hasher: &mut Hasher, seed: u64) -> Result<(), SimFailed> {
    let Some(shape) = NAMELESS.get(index(stream, 5)?) else {
        return hold(seed, false, "a shape draw falls inside the nameless shapes");
    };
    let mut path = PathBuf::from(*shape);
    for _ in 0..draw(stream, 4)? {
        path = Path::new("..").join(path);
    }
    hold(
        seed,
        path.file_name().is_none(),
        "the drawn shape has no file name",
    )?;
    hold(
        seed,
        successor_path(&path) == path,
        "a nameless path continues into itself",
    )?;
    hold(
        seed,
        generation_of(&path) == 1,
        "a nameless path is generation one",
    )?;
    hold(
        seed,
        !is_generation_path(&path),
        "a nameless path is no generation",
    )?;
    hasher.update(path.to_string_lossy().as_bytes());
    Ok(())
}

/// A trigger at or past either ceiling is refused; one under both is kept exactly as asked.
fn policies(stream: &mut Seeded, hasher: &mut Hasher, seed: u64) -> Result<(), SimFailed> {
    let events_at = SHIPPED_EVENT_CEILING
        .saturating_sub(10)
        .saturating_add(draw(stream, 21)?);
    let bytes_at = SHIPPED_BYTE_CEILING
        .saturating_sub(10)
        .saturating_add(draw(stream, 21)?);
    let fits = events_at < SHIPPED_EVENT_CEILING && bytes_at < SHIPPED_BYTE_CEILING;
    let kept = match ContinuationPolicy::new(events_at, bytes_at) {
        Ok(policy) => fits && (policy.events_at(), policy.bytes_at()) == (events_at, bytes_at),
        Err(_) => !fits,
    };
    hold(
        seed,
        kept,
        "a policy is kept exactly under both ceilings and refused otherwise",
    )?;
    hasher.update(&events_at.to_be_bytes());
    hasher.update(&bytes_at.to_be_bytes());
    hasher.update(&[u8::from(fits)]);
    Ok(())
}

/// A measured watermark is due exactly when either count reaches its point; an inert one never is.
fn watermarks(stream: &mut Seeded, hasher: &mut Hasher, seed: u64) -> Result<(), SimFailed> {
    let events_at = draw(stream, 100_000)?;
    let bytes_at = draw(stream, 1_000_001)?;
    let Ok(policy) = ContinuationPolicy::new(events_at, bytes_at) else {
        return hold(seed, false, "a policy under both ceilings is accepted");
    };
    let events = draw(stream, 120_001)?;
    let bytes = draw(stream, 1_200_001)?;
    let mark = ContinuationWatermark::measured(
        events,
        SHIPPED_EVENT_CEILING,
        bytes,
        SHIPPED_BYTE_CEILING,
        policy,
    );
    let due = events >= events_at || bytes >= bytes_at;
    hold(seed, mark.is_due() == due, "due exactly at either point")?;
    let read = (mark.events(), mark.bytes());
    let measured = (
        (events, SHIPPED_EVENT_CEILING),
        (bytes, SHIPPED_BYTE_CEILING),
    );
    hold(
        seed,
        read == measured,
        "the watermark reads back the counts it measured",
    )?;
    let inert =
        ContinuationWatermark::inert(events, SHIPPED_EVENT_CEILING, bytes, SHIPPED_BYTE_CEILING);
    hold(seed, !inert.is_due(), "an inert journal is never due")?;
    hasher.update(&events.to_be_bytes());
    hasher.update(&bytes.to_be_bytes());
    hasher.update(&[u8::from(due)]);
    Ok(())
}

#[test]
fn sim_every_base_name_continues_into_generation_two_inside_its_cont_directory()
-> Result<(), SimFailed> {
    replays(base_names)
}

#[test]
fn sim_a_chain_of_continuations_counts_one_generation_per_step_at_a_fixed_width()
-> Result<(), SimFailed> {
    replays(chains)
}

#[test]
fn sim_a_six_digit_base_outside_a_cont_directory_is_never_a_generation() -> Result<(), SimFailed> {
    replays(six_digit_bases)
}

#[test]
fn sim_a_path_with_no_file_name_continues_into_itself() -> Result<(), SimFailed> {
    replays(nameless_paths)
}

#[test]
fn sim_a_policy_at_or_past_either_ceiling_is_refused() -> Result<(), SimFailed> {
    replays(policies)
}

#[test]
fn sim_a_measured_watermark_is_due_exactly_when_a_count_reaches_its_point() -> Result<(), SimFailed>
{
    replays(watermarks)
}

/// Two seeds draw two different traces, so the hash is a hash of the draws.
#[test]
fn sim_naming_distinct_seeds_diverge() -> Result<(), SimFailed> {
    hold(
        2,
        trace(chains, 1)? != trace(chains, 2)?,
        "two seeds share a trace hash",
    )
}
