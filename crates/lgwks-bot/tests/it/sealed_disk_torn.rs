//! Ported torn-tail cases over the real sealed journals (#819).
//!
//! The kernel `tape-store` torn-tail family, re-targeted at
//! [`FileJournal`](lgwks_bot::journal::FileJournal) sealed files through the
//! public interface only: `open_sealed`, `compare_and_append_sealed`,
//! `sealed_entries` and `verify_sealed_chain`. The simulation that sweeps
//! these shapes lives beside them in `sim_sealed_disk.rs`; these are the
//! fixed cases it generalizes.

use std::error::Error;
use std::sync::Arc;

use lgwks_bot::effect::EffectKey;
use lgwks_bot::journal::{FileJournal, JournalError};
use lgwks_bot::sealed::keys::KeyProvider;

use crate::scratch::Scratch;
use crate::sealed_harness::{
    FaultingProvider, append_journal_event, assert_journal_chain, journal_event, journal_key,
    live_journal_keys,
};
use crate::sweep_fixtures::refuse;

type TestResult = Result<(), Box<dyn Error>>;

/// Tenant labels: file-safe, distinct, fixed across seeds.
const TENANT_A: &str = "disk-a";
const TENANT_B: &str = "disk-b";

/// Attempt namespace stride per tenant: the fixture fixes run, action,
/// environment and flow, so the attempt number is what keeps two tenants'
/// keys disjoint.
const ATTEMPT_STRIDE: u64 = 1_000_000;

/// Append `count` sealed events with attempt numbers from `base`, returning
/// their keys. Boundaries are the caller's to measure: every open starts
/// from a dropped handle, so no lock survives between phases.
fn append_numbered(
    path: &std::path::Path,
    provider: &Arc<dyn KeyProvider>,
    tenant: &str,
    base: u64,
    count: u64,
) -> Result<Vec<EffectKey>, Box<dyn Error>> {
    let mut journal = FileJournal::open_sealed(path, Arc::clone(provider), tenant)?;
    let mut keys = Vec::new();
    for offset in 0..count {
        let key = journal_key(&base.saturating_add(offset).to_string())?;
        append_journal_event(&mut journal, &journal_event(key))?;
        keys.push(key);
    }
    drop(journal);
    Ok(keys)
}

/// Cut the last byte off `path` and reopen it under `tenant`.
fn cut_tail_and_open(
    path: &std::path::Path,
    provider: &Arc<dyn KeyProvider>,
    tenant: &str,
) -> Result<(FileJournal, Vec<EffectKey>), Box<dyn Error>> {
    let mut torn = std::fs::read(path)?;
    let cut = torn.len().saturating_sub(1);
    torn.truncate(cut);
    std::fs::write(path, &torn)?;
    let journal = FileJournal::open_sealed(path, Arc::clone(provider), tenant)?;
    let got = live_journal_keys(&journal)?;
    Ok((journal, got))
}

/// A torn sealed tail repairs on open to the kept prefix (ports
/// `torn_tail_repaired_on_open`: the torn frame was never acknowledged, so
/// the open truncates back to the last whole frame and says so).
#[test]
fn sealed_torn_tail_repaired_on_open() -> TestResult {
    let dir = Scratch::new("sealed-disk-port")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(dir.path())?);
    let path = dir.path().join("a.jrnl");
    let keys = append_numbered(&path, &provider, TENANT_A, 1, 2)?;
    let (journal, got) = cut_tail_and_open(&path, &provider, TENANT_A)?;
    assert!(
        journal.torn_tail_repaired(),
        "a cut tail must report its repair"
    );
    assert_eq!(
        got,
        keys[..1],
        "a torn tail truncates to the last whole frame"
    );
    assert_journal_chain(&journal)?;
    Ok(())
}

/// A refused open moves no byte, and a later open still repairs (ports
/// `torn_tail_budget_refusal_is_non_destructive`: the refusal lands before
/// any write, and the torn tail it refused to touch is still repairable).
#[test]
fn sealed_torn_tail_refusal_is_non_destructive() -> TestResult {
    let dir = Scratch::new("sealed-disk-port")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(dir.path())?);
    let path = dir.path().join("a.jrnl");
    let keys = append_numbered(&path, &provider, TENANT_A, 1, 3)?;
    let good = std::fs::read(&path)?;
    if good.is_empty() {
        return refuse("three sealed appends left no bytes");
    }
    let mut bad = good.clone();
    let at = good.len().saturating_sub(40);
    bad[at] ^= 0xFF;
    std::fs::write(&path, &bad)?;
    match FileJournal::open_sealed(&path, Arc::clone(&provider), TENANT_A) {
        Err(JournalError::Corrupt(_)) => {}
        Err(other) => {
            return refuse(format!(
                "a lying sealed frame refused as {other:?}, not Corrupt"
            ));
        }
        Ok(_) => {
            return refuse("a lying sealed frame opened");
        }
    }
    let still = std::fs::read(&path)?;
    if still != bad {
        return refuse("the refused open moved a byte of the corrupt file");
    }
    std::fs::write(&path, &good)?;
    let (journal, got) = cut_tail_and_open(&path, &provider, TENANT_A)?;
    assert!(
        journal.torn_tail_repaired(),
        "the refused file still repairs once torn"
    );
    assert_eq!(
        got,
        keys[..2],
        "the repair keeps the acknowledged prefix behind the tear"
    );
    assert_journal_chain(&journal)?;
    Ok(())
}

/// A long torn tail repairs its whole kept prefix (ports
/// `large_torn_tail_budget_accounts_for_plan_capacity`: the scan is bounded
/// by the file's own length, so a long history repairs frame by frame).
#[test]
fn large_sealed_torn_tail_repairs_whole_prefix() -> TestResult {
    let dir = Scratch::new("sealed-disk-port")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(dir.path())?);
    let path = dir.path().join("a.jrnl");
    let keys = append_numbered(&path, &provider, TENANT_A, 1, 65)?;
    let (journal, got) = cut_tail_and_open(&path, &provider, TENANT_A)?;
    assert!(
        journal.torn_tail_repaired(),
        "a 65-frame torn tail must report its repair"
    );
    assert_eq!(
        got,
        keys[..64],
        "a long tear keeps every whole frame behind it"
    );
    assert_journal_chain(&journal)?;
    Ok(())
}

/// A file past the byte ceiling refuses without moving a byte: capacity is
/// a refusal, never a trim.
#[test]
fn sealed_over_ceiling_open_refuses_without_truncating() -> TestResult {
    use lgwks_bot::journal::MAX_JOURNAL_BYTES;
    let dir = Scratch::new("sealed-disk-port")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(dir.path())?);
    let path = dir.path().join("a.jrnl");
    let oversized = MAX_JOURNAL_BYTES.saturating_add(1);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    file.set_len(oversized)?;
    drop(file);
    match FileJournal::open_sealed(&path, Arc::clone(&provider), TENANT_A) {
        Err(JournalError::CapacityExceeded { .. }) => {}
        Err(other) => {
            return refuse(format!(
                "an over-ceiling sealed file refused as {other:?}, not CapacityExceeded"
            ));
        }
        Ok(_) => {
            return refuse("an over-ceiling sealed file opened");
        }
    }
    let len = std::fs::metadata(&path)?.len();
    if len != oversized {
        return refuse("the refused over-ceiling open moved a byte");
    }
    Ok(())
}

/// One tenant's torn tail never moves the other's journal, and neither
/// tenant opens the other's file (ports
/// `torn_tail_repair_isolated_between_tenants`).
#[test]
fn sealed_torn_tail_isolation_between_tenants() -> TestResult {
    let dir = Scratch::new("sealed-disk-port")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(FaultingProvider::open(dir.path())?);
    let path_a = dir.path().join("a.jrnl");
    let path_b = dir.path().join("b.jrnl");
    let keys_a = append_numbered(&path_a, &provider, TENANT_A, 1, 3)?;
    let keys_b = append_numbered(&path_b, &provider, TENANT_B, ATTEMPT_STRIDE, 3)?;
    let good_b = std::fs::read(&path_b)?;
    let (journal_a, got_a) = cut_tail_and_open(&path_a, &provider, TENANT_A)?;
    assert!(
        journal_a.torn_tail_repaired(),
        "tenant A's torn tail must report its repair"
    );
    assert_eq!(
        got_a,
        keys_a[..2],
        "tenant A's repair keeps tenant A's kept prefix"
    );
    drop(journal_a);
    let journal_b = FileJournal::open_sealed(&path_b, Arc::clone(&provider), TENANT_B)?;
    assert!(
        !journal_b.torn_tail_repaired(),
        "tenant B's journal was never torn"
    );
    let got_b = live_journal_keys(&journal_b)?;
    assert_eq!(got_b, keys_b, "tenant B reads all three of its records");
    drop(journal_b);
    let still_b = std::fs::read(&path_b)?;
    if still_b != good_b {
        return refuse("tenant A's repair moved a byte of tenant B's file");
    }
    let before_foreign = std::fs::read(&path_a)?;
    match FileJournal::open_sealed(&path_a, Arc::clone(&provider), TENANT_B) {
        Err(JournalError::Corrupt(_)) => {}
        Err(other) => {
            return refuse(format!(
                "tenant B's open of tenant A's file refused as {other:?}, not Corrupt"
            ));
        }
        Ok(_) => {
            return refuse("tenant B read tenant A's file");
        }
    }
    match FileJournal::open_sealed(&path_b, Arc::clone(&provider), TENANT_A) {
        Err(JournalError::Corrupt(_)) => {}
        Err(other) => {
            return refuse(format!(
                "tenant A's open of tenant B's file refused as {other:?}, not Corrupt"
            ));
        }
        Ok(_) => {
            return refuse("tenant A read tenant B's file");
        }
    }
    let after_foreign = std::fs::read(&path_a)?;
    if before_foreign != after_foreign {
        return refuse("the refused foreign opens moved a byte of tenant A's file");
    }
    let journal = FileJournal::open_sealed(&path_a, Arc::clone(&provider), TENANT_A)?;
    let again = live_journal_keys(&journal)?;
    if again != keys_a[..2] {
        return refuse("tenant A's file changed under the refused foreign opens");
    }
    Ok(())
}
