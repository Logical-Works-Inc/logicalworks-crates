//! Simulation family: the mechanism a continuation rests on, one seeded property
//! per test.
//!
//! `sim_continuation` drives the *lifecycle* — long runs, carries, tenants and a
//! reopen against an oracle. This layer pins the *mechanism* underneath it, each
//! property over its own band so a failure names the property rather than a run:
//! where the watermark falls, what a successor is called and chains from, what the
//! seal writes, what a sealed or half-sealed chain refuses, what a checkpoint holds
//! at its two bounds, and what a reopen reads back. `journal_continuation.rs` states
//! each of these once, at one hand-picked shape; here the seed picks the shape and
//! the property has to hold for every one it picks.
//!
//! Every property drives the real [`FileJournal`] on a real directory through its
//! public doors, and every band is swept twice through [`sim::assert_replays`], so a
//! run whose trace depends on anything but its seed fails even when every assertion
//! passes.
//!
//! The properties live in four children by responsibility — `watermark`, `chain`,
//! `checkpoint` and `reopen` — and this file keeps what they share (the helpers and
//! the bands) and the sweep that runs each of them.

mod chain;
mod checkpoint;
mod reopen;
mod watermark;

use crate::sim;
use crate::sim_continuation::{
    Fate, Oracle, Pending, TestResult, action_of, assert_fold_agrees, continue_if_due, key_of,
    ladder, status_of, verdict,
};

use std::collections::BTreeSet;
use std::error::Error;
use std::path::Path;

use lgwks_bot::journal::{
    AttemptStatus, ContinuationPolicy, EffectEvent, EffectEvidence, EffectJournal, FileJournal,
    JournalError, JournalLimitKind, MAX_CHECKPOINT_SETTLED, MAX_CHECKPOINT_UNRESOLVED, SealPause,
    verify_chain_from,
};

use sim::Band;

/// Seeds each property sweeps, twice each.
///
/// Every property costs a few dozen flushes per seed, and what it pins is a
/// mechanism whose shape the seed varies inside one draw: three draws cover the
/// shapes without making the family a measurement of the device's flush rate.
const SEEDS: u64 = 3;

/// The first seed of this layer, clear of every other family's bands.
const FIRST_SEED: u64 = 5_000;

/// The byte trigger every family that is not about bytes opens with.
///
/// Far below the frame ceiling and far above anything these families write, so
/// only the event trigger or an explicit seal ever continues them.
const QUIET_BYTES: u64 = 8 * 1024 * 1024;

/// The band of property `index`, so no two properties share a seed.
const fn band(index: u64) -> Band {
    Band::new(
        FIRST_SEED.saturating_add(index.saturating_mul(SEEDS)),
        SEEDS,
    )
}

/// A continuing journal at `path`, triggered at `events_at` events or `bytes_at`
/// bytes, whichever comes first.
fn continuing(path: &Path, events_at: u64, bytes_at: u64) -> Result<FileJournal, Box<dyn Error>> {
    let policy = ContinuationPolicy::new(events_at, bytes_at)?;
    Ok(FileJournal::open_continuing_with(path, policy)?)
}

/// A continuing journal whose own trigger never falls inside a family, so every
/// continuation is one the family asked for.
fn quiet(path: &Path) -> Result<FileJournal, Box<dyn Error>> {
    Ok(FileJournal::open_continuing_with(
        path,
        ContinuationPolicy::declared(),
    )?)
}

/// Seal `journal` and hand back its successor.
fn seal(journal: &mut FileJournal) -> Result<FileJournal, Box<dyn Error>> {
    Ok(journal
        .continue_as_file()?
        .ok_or("a continuing journal must hand back a successor")?)
}

/// Write `count` attempts of `action`, numbered from `first`, each walked to the
/// fate `next` names for it.
fn write_with(
    journal: &mut FileJournal,
    oracle: &mut Oracle,
    action: u64,
    (first, count): (u64, u64),
    mut next: impl FnMut() -> Fate,
) -> TestResult {
    let mut pending = Pending::new(action);
    let last = first.saturating_add(count).saturating_sub(1);
    for number in first..=last {
        pending.push(journal, oracle, number, next(), number == last)?;
    }
    Ok(())
}

/// Write `count` attempts of `action` from `first`, each walked to `fate`.
fn write(
    journal: &mut FileJournal,
    oracle: &mut Oracle,
    action: u64,
    attempts: (u64, u64),
    fate: Fate,
) -> TestResult {
    write_with(journal, oracle, action, attempts, || fate)
}

/// Write `count` attempts of `action` from `first`, each to a fate the seed draws.
fn write_drawn(
    sim: &mut sim::Sim,
    journal: &mut FileJournal,
    oracle: &mut Oracle,
    action: u64,
    attempts: (u64, u64),
) -> TestResult {
    write_with(journal, oracle, action, attempts, || drawn(sim))
}

/// A fate the seed draws, unknown one time in eight.
///
/// Rare enough that no family's carry approaches [`MAX_CHECKPOINT_UNRESOLVED`]
/// unless the family is about that bound.
fn drawn(sim: &mut sim::Sim) -> Fate {
    match sim.rng().below(8) {
        0 => Fate::Unknown,
        1 | 2 => Fate::NotApplied,
        3 => Fate::VerificationFailed,
        _ => Fate::Applied,
    }
}

/// A settled fate the seed draws.
fn settled(sim: &mut sim::Sim) -> Fate {
    match sim.rng().below(3) {
        0 => Fate::NotApplied,
        1 => Fate::VerificationFailed,
        _ => Fate::Applied,
    }
}

/// A fresh intent for `attempt` of `action`.
fn intent(action: u64, attempt: u64) -> Result<EffectEvent, Box<dyn Error>> {
    Ok(EffectEvent::IntentAdmitted {
        key: key_of(action, attempt)?,
    })
}

/// The file's length on the disk.
fn length(path: &Path) -> Result<u64, Box<dyn Error>> {
    Ok(std::fs::metadata(path)?.len())
}

/// A draw in `low..low + span`.
fn draw(sim: &mut sim::Sim, low: u64, span: u32) -> u64 {
    low.saturating_add(u64::from(sim.rng().below(span)))
}

/// A refused continuation moved no byte of `journal` and wrote no successor.
fn assert_unwritten(journal: &FileJournal, before: u64) -> TestResult {
    assert_eq!(
        length(journal.path())?,
        before,
        "refused before a byte moved"
    );
    assert!(
        !journal.successor_path().exists(),
        "and no successor exists"
    );
    Ok(())
}

// ── The sweep ─────────────────────────────────────────────────────────────

#[test]
fn the_event_watermark_falls_on_the_flush_that_reaches_it() -> TestResult {
    sim::assert_replays(band(0), watermark::event_watermark)
}

#[test]
fn the_byte_watermark_alone_makes_a_continuation_due() -> TestResult {
    sim::assert_replays(band(1), watermark::byte_watermark)
}

#[test]
fn a_journal_without_a_lifecycle_is_never_due() -> TestResult {
    sim::assert_replays(band(2), watermark::never_due)
}

#[test]
fn the_headroom_falls_by_exactly_what_each_flush_appended() -> TestResult {
    sim::assert_replays(band(3), watermark::headroom)
}

#[test]
fn every_successor_is_the_next_fixed_width_name_beside_its_base() -> TestResult {
    sim::assert_replays(band(4), chain::generation_names)
}

#[test]
fn a_successor_chains_from_the_tail_its_predecessor_sealed() -> TestResult {
    sim::assert_replays(band(5), chain::chain_base)
}

#[test]
fn a_reopen_through_the_base_reaches_the_live_generation() -> TestResult {
    sim::assert_replays(band(6), chain::reopen_reaches_live)
}

#[test]
fn a_base_named_like_a_generation_is_refused_before_it_exists() -> TestResult {
    sim::assert_replays(band(7), chain::ambiguous_base)
}

#[test]
fn a_seal_is_one_frame_byte_identical_in_both_files() -> TestResult {
    sim::assert_replays(band(8), chain::seal_bytes)
}

#[test]
fn a_sealed_predecessor_refuses_everything_and_keeps_its_bytes() -> TestResult {
    sim::assert_replays(band(9), chain::sealed_refuses)
}

#[test]
fn a_seal_stopped_at_any_boundary_leaves_one_authority() -> TestResult {
    sim::assert_replays(band(10), chain::stopped_seal)
}

#[test]
fn the_carry_bound_admits_its_size_and_refuses_one_more_unwritten() -> TestResult {
    sim::assert_replays(band(11), checkpoint::carry_bound)
}

#[test]
fn the_action_bound_admits_its_size_and_refuses_one_more_unwritten() -> TestResult {
    sim::assert_replays(band(12), checkpoint::action_bound)
}

#[test]
fn a_checkpoint_holds_one_record_per_action_whatever_the_history() -> TestResult {
    sim::assert_replays(band(13), checkpoint::one_record_per_action)
}

#[test]
fn every_walked_attempt_is_refused_after_a_continuation() -> TestResult {
    sim::assert_replays(band(14), checkpoint::older_refused)
}

#[test]
fn a_newer_attempt_of_every_folded_action_is_admitted() -> TestResult {
    sim::assert_replays(band(15), checkpoint::newer_admitted)
}

#[test]
fn a_failed_verification_crosses_every_continuation_unchanged() -> TestResult {
    sim::assert_replays(band(16), checkpoint::verification_crosses)
}

#[test]
fn an_attempt_settled_in_a_successor_stays_settled_after_the_next() -> TestResult {
    sim::assert_replays(band(17), checkpoint::settled_later)
}

#[test]
fn a_not_applied_attempt_is_carried_and_its_retry_admitted() -> TestResult {
    sim::assert_replays(band(18), checkpoint::not_applied_retry)
}

#[test]
fn every_generation_streams_exactly_what_it_committed() -> TestResult {
    sim::assert_replays(band(19), reopen::replay_agrees)
}

#[test]
fn every_acknowledged_position_reads_back_after_a_reopen() -> TestResult {
    sim::assert_replays(band(20), reopen::positions_read_back)
}

#[test]
fn interleaved_tenants_continue_on_their_own_triggers_and_never_cross() -> TestResult {
    sim::assert_replays(band(21), reopen::tenants_interleaved)
}

#[test]
fn the_widest_carry_both_bounds_admit_is_sealed_twice() -> TestResult {
    sim::assert_replays(band(25), checkpoint::widest_carry)
}

#[test]
fn a_lengthened_event_behind_a_carried_seal_is_refused_on_every_seed() -> TestResult {
    sim::assert_replays(band(26), reopen::lengthened_behind_seal)
}

/// The same seed replays to one hash and distinct seeds reach distinct ones.
///
/// `assert_replays` proves the first half for every family above; the second
/// half is what keeps it honest, because a family whose trace ignored its seed
/// would replay perfectly while testing one shape.
#[test]
fn the_same_seed_replays_and_distinct_seeds_diverge() -> TestResult {
    let wide = Band::new(FIRST_SEED.saturating_add(22_u64.saturating_mul(SEEDS)), 8);
    let first = sim::sweep(wide, chain::generation_names)?;
    let second = sim::sweep(wide, chain::generation_names)?;
    assert_eq!(first, second, "one seed, one trace hash");
    let distinct: BTreeSet<u64> = first.iter().copied().collect();
    assert!(
        distinct.len() > 1,
        "eight seeds reached {} distinct trace(s), so the trace does not depend on the seed",
        distinct.len()
    );
    Ok(())
}
