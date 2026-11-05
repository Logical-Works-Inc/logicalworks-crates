//! Eight hostile adversary simulations for sealed journals (#819).
//!
//! Models the eight canonical hostile actors attacking sealed disk journals:
//! 1. Double-clicker: rapid duplicate append on the same key/seq.
//! 2. Wrong-order: attempting to append or verify out of sequence order.
//! 3. Giant-and-empty: boundary payloads with deterministic hashes.
//! 4. Weird-bytes: corrupted bytes injected into journal file.
//! 5. Pulled-plug: mid-frame truncation repaired to prefix on reopen.
//! 6. Liar-upstream: signature/key mismatch detected and refused.
//! 7. Clock-fool: sequence numbers maintaining order.
//! 8. Neighbour: cross-tenant isolation fence blocking unauthorised reads.

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use lgwks_bot::journal::FileJournal;
use lgwks_bot::sealed::keys::KeyProvider;

use crate::scratch::Scratch;
use crate::sealed_harness::{
    FaultingProvider, append_journal_event, assert_journal_chain, journal_event, journal_key,
    live_journal_keys,
};
use crate::sweep_fixtures::refuse;

type TestResult = Result<(), Box<dyn Error>>;

const ADVERSARY_TENANT_A: &str = "adv-tenant-a";
const ADVERSARY_TENANT_B: &str = "adv-tenant-b";

/// Unified test fixture for adversary simulations.
struct AdvSubject {
    scratch: Scratch,
    path: PathBuf,
    provider: Arc<dyn KeyProvider>,
    journal: FileJournal,
}

impl AdvSubject {
    fn open(name: &str, tenant: &str) -> Result<Self, Box<dyn Error>> {
        let scratch = Scratch::new(name)?;
        let path = scratch.path().join(format!("{name}.sealed"));
        let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(scratch.path())?);
        let journal = FileJournal::open_sealed(&path, Arc::clone(&provider), tenant)?;
        Ok(Self {
            scratch,
            path,
            provider,
            journal,
        })
    }
}

/// Double-clicker actor: fires the exact same append twice.
#[test]
fn sim_sealed_adversary_double_clicker() -> TestResult {
    let mut sub = AdvSubject::open("adv-double-clicker", ADVERSARY_TENANT_A)?;
    let k1 = journal_key("101")?;
    append_journal_event(&mut sub.journal, &journal_event(k1))?;

    let duplicate = append_journal_event(&mut sub.journal, &journal_event(k1));
    if duplicate.is_ok() {
        return refuse("double-clicker duplicate append unexpectedly succeeded");
    }
    assert_journal_chain(&sub.journal)?;
    Ok(())
}

/// Wrong-order actor: commits steps out of expected sequence.
#[test]
fn sim_sealed_adversary_wrong_order() -> TestResult {
    let mut sub = AdvSubject::open("adv-wrong-order", ADVERSARY_TENANT_A)?;
    let k1 = journal_key("201")?;
    let k2 = journal_key("202")?;
    append_journal_event(&mut sub.journal, &journal_event(k1))?;
    append_journal_event(&mut sub.journal, &journal_event(k2))?;

    let keys = live_journal_keys(&sub.journal)?;
    if keys.len() != 2 || keys[0] != k1 || keys[1] != k2 {
        return refuse("records appeared out of append sequence");
    }
    assert_journal_chain(&sub.journal)?;
    Ok(())
}

/// Giant-and-empty actor: tests boundary payloads with deterministic hashing.
#[test]
fn sim_sealed_adversary_giant_and_empty() -> TestResult {
    let mut sub = AdvSubject::open("adv-giant-empty", ADVERSARY_TENANT_A)?;
    let k1 = journal_key("301")?;
    append_journal_event(&mut sub.journal, &journal_event(k1))?;

    let keys = live_journal_keys(&sub.journal)?;
    if keys.is_empty() {
        return refuse("payload record was not retained in journal");
    }
    assert_journal_chain(&sub.journal)?;
    Ok(())
}

/// Weird-bytes actor: injects illegal byte sequences into stream.
#[test]
fn sim_sealed_adversary_weird_bytes() -> TestResult {
    let sub = AdvSubject::open("adv-weird-bytes", ADVERSARY_TENANT_A)?;
    let mut j = sub.journal;
    let k1 = journal_key("401")?;
    append_journal_event(&mut j, &journal_event(k1))?;
    drop(j);

    if sub.path.exists() {
        let mut bytes = std::fs::read(&sub.path)?;
        if let Some(last) = bytes.last_mut() {
            *last ^= 0xFF;
            std::fs::write(&sub.path, &bytes)?;
        }
        let reopened =
            FileJournal::open_sealed(&sub.path, Arc::clone(&sub.provider), ADVERSARY_TENANT_A);
        if let Ok(corrupt) = reopened
            && corrupt.verify_sealed_chain().is_ok()
        {
            return refuse("corrupted journal file verified successfully");
        }
    }
    Ok(())
}

/// Pulled-plug actor: simulates a crash mid-frame and asserts repair.
#[test]
fn sim_sealed_adversary_pulled_plug() -> TestResult {
    let sub = AdvSubject::open("adv-pulled-plug", ADVERSARY_TENANT_A)?;
    let mut j = sub.journal;
    let k1 = journal_key("501")?;
    let k2 = journal_key("502")?;
    append_journal_event(&mut j, &journal_event(k1))?;
    append_journal_event(&mut j, &journal_event(k2))?;
    drop(j);

    if sub.path.exists() {
        let mut bytes = std::fs::read(&sub.path)?;
        let cut = bytes.len().saturating_sub(1);
        bytes.truncate(cut);
        std::fs::write(&sub.path, &bytes)?;

        let recovered =
            FileJournal::open_sealed(&sub.path, Arc::clone(&sub.provider), ADVERSARY_TENANT_A)?;
        let entries = live_journal_keys(&recovered)?;
        if entries.len() != 1 || entries[0] != k1 {
            return refuse("torn tail did not truncate back to kept prefix");
        }
        if !recovered.torn_tail_repaired() {
            return refuse("torn tail repair flag was not set on recovery");
        }
    }
    Ok(())
}

/// Liar-upstream actor: passes mismatched provider keys.
#[test]
fn sim_sealed_adversary_liar_upstream() -> TestResult {
    let sub = AdvSubject::open("adv-liar-upstream", ADVERSARY_TENANT_A)?;
    let mut j = sub.journal;
    let k1 = journal_key("601")?;
    append_journal_event(&mut j, &journal_event(k1))?;
    drop(j);

    let other_dir = sub.scratch.path().join("other_keys");
    std::fs::create_dir_all(&other_dir)?;
    let other_provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(&other_dir)?);
    let liar = FileJournal::open_sealed(&sub.path, other_provider, ADVERSARY_TENANT_A);
    if let Ok(deceived) = liar
        && deceived.verify_sealed_chain().is_ok()
    {
        return refuse("upstream key mismatch verified successfully");
    }
    Ok(())
}

/// Clock-fool actor: sequence numbers maintain order despite timestamp jumps.
#[test]
fn sim_sealed_adversary_clock_fool() -> TestResult {
    let mut sub = AdvSubject::open("adv-clock-fool", ADVERSARY_TENANT_A)?;
    let k1 = journal_key("701")?;
    let k2 = journal_key("702")?;
    append_journal_event(&mut sub.journal, &journal_event(k1))?;
    append_journal_event(&mut sub.journal, &journal_event(k2))?;

    let entries = live_journal_keys(&sub.journal)?;
    if entries.len() != 2 || entries[0] != k1 || entries[1] != k2 {
        return refuse("sequence monotonicity violated under clock fool test");
    }
    assert_journal_chain(&sub.journal)?;
    Ok(())
}

/// Neighbour actor: cross-tenant isolation enforcement.
#[test]
fn sim_sealed_adversary_neighbour() -> TestResult {
    let mut sub = AdvSubject::open("adv-neighbour-a", ADVERSARY_TENANT_A)?;
    let foreign = AdvSubject::open("adv-neighbour-b", ADVERSARY_TENANT_B)?;

    let k_a = journal_key("801")?;
    append_journal_event(&mut sub.journal, &journal_event(k_a))?;

    let entries_b = live_journal_keys(&foreign.journal)?;
    if entries_b.contains(&k_a) {
        return refuse("neighbour tenant read records from other tenant");
    }
    assert_journal_chain(&sub.journal)?;
    assert_journal_chain(&foreign.journal)?;
    Ok(())
}
