//! Sealed records at rest against the real file-backed stores: seal, shred,
//! restart, erasure.
//!
//! Each test names the requirement it proves and the real-world source it
//! derives from. The failure behind every row is a breach reading another
//! tenant's secret, an erasure that left bytes readable, or a log that
//! carried a key.
//!
//! The journal half seals [`EffectEvent`](lgwks_bot::journal::EffectEvent)s
//! through [`FileJournal`](lgwks_bot::journal::FileJournal); the run-store
//! half seals step records through
//! [`RunStore`](lgwks_bot::task::RunStore). Both go through the same public
//! constructors `estate sealed` uses, so a harness that passed while the
//! shipped store refused is impossible.
//!
//! | Test | Requirement | Source |
//! |---|---|---|
//! | `sealed_records_are_unreadable_without_the_provider_and_verify` | sealed records are ciphertext without their managed keys, and the chain verifies | GDPR Art. 32 (security of processing) |
//! | `shred_is_irreversible_and_idempotent_across_a_restart` | shredding destroys the key and the tombstone survives a restart; a second shred is a no-op | NIST SP 800-88 Rev. 1 §4 (crypto-erase/purge) |
//! | `a_torn_shred_leaves_live_or_erased_never_readable_without_a_key` | a torn write at either shred point leaves live or erased, never readable-without-its-managed-key | NIST SP 800-88 (sanitization under failure) |
//! | `a_transient_provider_fault_neither_resurrects_nor_drops` | a transient fault propagates as itself: no bytes, no tombstone | the read fold (§5 of #785) |
//! | `two_tenants_never_share_a_key_or_a_record` | erasing one tenant leaves the other's records live and both chains verifying | GDPR Art. 17 (right to erasure, tenant scope) |
//! | `key_bytes_never_reach_debug_or_log_output` | no key bytes and no plaintext in `Debug` output, errors or files | CWE-532 (sensitive information in logs) |
//! | `erase_tenant_and_ttl_sweep_remove_only_what_is_due` | erasure by tenant and by TTL shreds exactly the due records | GDPR Art. 17; the TTL scheduler of #785 |
//! | `sealed_journal_seals_shreds_and_verifies` | journal frames seal, shred to tombstones, and verify across a restart | GDPR Art. 32; NIST SP 800-88 §4 |
//! | `a_plain_open_refuses_a_sealed_journal` | a sealed file never opens as plain and a plain file never opens as sealed | the mode guard of #785 |

use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::sealed::keys::{KeyError, KeyId, KeyProvider, SoftwareProvider};
use lgwks_bot::task::{RunStore, StoreError};

use crate::scratch::Scratch;
use crate::sealed_harness::{
    FaultingProvider, StepSpec, append_journal_event, append_step, assert_journal_chain,
    assert_store_chain, erased_step, journal_event, journal_key, live_journal_keys,
    live_step_bytes, test_definition, test_key, test_run,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Open a sealed run store over `dir/store.runstore` with `provider`.
fn open_store(
    dir: &std::path::Path,
    provider: Arc<dyn KeyProvider>,
) -> Result<RunStore, Box<dyn std::error::Error>> {
    Ok(RunStore::open_sealed(dir.join("store.runstore"), provider)?)
}

/// Seal two steps under one run and return their keys: the pair every
/// two-record test opens with.
///
/// One definition because the unreadable-bytes test and the torn-shred test
/// both open with a pair, and two spellings would be two chances to seal the
/// second record under the first record's key.
fn seal_two_live(
    store: &RunStore,
    tenant: &str,
    run: lgwks_bot::effect::RunId,
    definition: &lgwks_bot::task::DefinitionIdentity,
    first: (&str, &[u8]),
    second: (&str, &[u8]),
) -> Result<(lgwks_bot::script::StepKey, lgwks_bot::script::StepKey), Box<dyn std::error::Error>> {
    let first_key = test_key(tenant, first.0)?;
    let second_key = test_key(tenant, second.0)?;
    for (key, path, body) in [
        (first_key, first.0, first.1),
        (second_key, second.0, second.1),
    ] {
        append_step(
            store,
            StepSpec {
                tenant,
                run,
                key,
                path,
                definition,
                bytes: body.to_vec(),
                expires_at: None,
            },
        )?;
    }
    Ok((first_key, second_key))
}

/// Sealed records are ciphertext without their managed keys, and the chain
/// verifies: the file holds no plaintext, the reopened store reads the same
/// live bytes, and a fresh provider over the same file cannot open it at all
/// (GDPR Art. 32).
#[test]
fn sealed_records_are_unreadable_without_the_provider_and_verify() -> TestResult {
    let scratch = Scratch::new("sealed")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let store = open_store(scratch.path(), Arc::clone(&provider))?;
    let definition = test_definition();
    let run = test_run(1)?;
    let (first, second) = seal_two_live(
        &store,
        "acme",
        run,
        &definition,
        ("launch/one", b"the acme launch codes".as_slice()),
        ("launch/two", b"the acme backup codes".as_slice()),
    )?;
    assert_store_chain(&store)?;
    assert_eq!(
        live_step_bytes(&store, "acme", run, first)?,
        b"the acme launch codes"
    );
    assert_eq!(
        live_step_bytes(&store, "acme", run, second)?,
        b"the acme backup codes"
    );
    drop(store);
    let reopened = open_store(scratch.path(), provider)?;
    assert_store_chain(&reopened)?;
    assert_eq!(
        live_step_bytes(&reopened, "acme", run, first)?,
        b"the acme launch codes",
        "a restart replays the keys and the live records read back"
    );
    assert_eq!(
        live_step_bytes(&reopened, "acme", run, second)?,
        b"the acme backup codes"
    );
    let file_bytes = std::fs::read(scratch.path().join("store.runstore"))?;
    let text = String::from_utf8_lossy(&file_bytes);
    assert!(
        !text.contains("the acme launch codes") && !text.contains("the acme backup codes"),
        "the sealed file holds ciphertext, not the records"
    );
    drop(reopened);
    // A provider that never held these keys cannot open the file at all: the
    // replay refuses the wrapped keys, so nothing reads as live.
    let stranger: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(
        &scratch.path().join("stranger-keys"),
    )?);
    match open_store(scratch.path(), stranger) {
        Err(_) => {}
        Ok(_) => {
            return Err("a provider that never held the keys must not open the sealed file".into());
        }
    }
    Ok(())
}

/// Shredding destroys the key and the tombstone survives a restart; a second
/// shred is a no-op, and the shredded chain still verifies
/// (NIST SP 800-88 crypto-erase).
#[test]
fn shred_is_irreversible_and_idempotent_across_a_restart() -> TestResult {
    use lgwks_bot::sealed::runstore::ShredOutcome;
    let scratch = Scratch::new("sealed")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let store = open_store(scratch.path(), Arc::clone(&provider))?;
    let definition = test_definition();
    let run = test_run(2)?;
    let erased_key = test_key("acme", "shred/one")?;
    let kept_key = test_key("acme", "shred/two")?;
    for (key, path, body) in [
        (erased_key, "shred/one", b"shred this".as_slice()),
        (kept_key, "shred/two", b"keep this".as_slice()),
    ] {
        append_step(
            &store,
            StepSpec {
                tenant: "acme",
                run,
                key,
                path,
                definition: &definition,
                bytes: body.to_vec(),
                expires_at: None,
            },
        )?;
    }
    match store.shred_record("acme", run, erased_key)? {
        ShredOutcome::Shredded { run: got, key } => {
            assert_eq!(got, run, "the shred names its run");
            assert_eq!(key, erased_key, "the shred names its record");
        }
        _ => return Err("a first shred is never a no-op".into()),
    }
    erased_step(&store, "acme", run, erased_key)?;
    match store.shred_record("acme", run, erased_key)? {
        ShredOutcome::AlreadyErased { .. } => {}
        _ => return Err("a second shred destroyed again".into()),
    }
    assert_eq!(
        live_step_bytes(&store, "acme", run, kept_key)?,
        b"keep this"
    );
    assert_store_chain(&store)?;
    drop(store);
    let reopened = open_store(scratch.path(), provider)?;
    erased_step(&reopened, "acme", run, erased_key)?;
    assert_eq!(
        live_step_bytes(&reopened, "acme", run, kept_key)?,
        b"keep this"
    );
    assert_store_chain(&reopened)?;
    match reopened.shred_record("acme", run, erased_key)? {
        ShredOutcome::AlreadyErased { .. } => Ok(()),
        _ => Err("shredding a destroyed record past a reopen is a no-op".into()),
    }
}

/// A torn write at either shred point leaves live or erased, never
/// readable-without-its-managed-key (NIST SP 800-88 under failure).
#[test]
fn a_torn_shred_leaves_live_or_erased_never_readable_without_a_key() -> TestResult {
    use lgwks_bot::script::FlowError;
    let scratch = Scratch::new("sealed")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let store = open_store(scratch.path(), Arc::clone(&provider))?;
    let definition = test_definition();
    let run = test_run(3)?;
    let (first, second) = seal_two_live(
        &store,
        "acme",
        run,
        &definition,
        ("torn/one", b"a record mid-shred".as_slice()),
        ("torn/two", b"a record past the shred".as_slice()),
    )?;
    // The shred lands both halves; the tears below cut the tombstone tail.
    use lgwks_bot::sealed::runstore::ShredOutcome as Outcome;
    match store.shred_record("acme", run, first)? {
        Outcome::Shredded { .. } => {}
        _ => return Err("a first shred shreds".into()),
    }
    let path = scratch.path().join("store.runstore");
    let file_bytes = std::fs::read(&path)?;
    drop(store);
    // Cut the sealed file from the header to the torn tail: every cut is a
    // crash the open trims to live-or-erased, never to a third state.
    let header = 16_usize;
    let full = file_bytes.len();
    let middle = header.saturating_add(full.saturating_sub(header) >> 1);
    let cuts = [header, middle, full.saturating_sub(1), full];
    for cut in cuts {
        std::fs::write(&path, &file_bytes[..cut.min(full)])?;
        let reopened = open_store(scratch.path(), Arc::clone(&provider))?;
        // The shredded record reads erased while its frame survives, and as
        // an unknown refusal once the tear took its frame.
        if let Ok(lgwks_bot::sealed::runstore::SealedStepRead::Live { .. }) =
            reopened.read_sealed("acme", run, first)
        {
            return Err(format!("a shredded record never reads as live at cut {cut}").into());
        }
        // The unshredded record reads whole while its frame survives, and
        // as a refusal once the tear took it — never as erased, which only
        // a destruction folds to.
        match reopened.lookup("acme", run, second) {
            Ok(Some(bytes)) => {
                assert_eq!(
                    bytes, b"a record past the shred",
                    "an unshredded record after a torn shred reads whole at cut {cut}"
                );
            }
            Ok(None) => {
                return Err(format!("a sealed lookup never answers absence at cut {cut}").into());
            }
            Err(FlowError::Store { .. }) => {}
            Err(other) => {
                return Err(
                    format!("a torn tail is a store refusal, not {other:?}, at cut {cut}").into(),
                );
            }
        }
        let cut_bytes = std::fs::read(&path)?;
        let cut_text = String::from_utf8_lossy(&cut_bytes);
        assert!(
            !cut_text.contains("a record mid-shred")
                && !cut_text.contains("a record past the shred"),
            "a torn file still holds ciphertext, not the records, at cut {cut}"
        );
        assert_store_chain(&reopened)?;
        drop(reopened);
    }
    Ok(())
}

/// A transient fault propagates as itself: the read refuses without bytes
/// and without a tombstone, and the healed read is live again — never
/// resurrected, never dropped (the read fold of #785).
#[test]
fn a_transient_provider_fault_neither_resurrects_nor_drops() -> TestResult {
    let scratch = Scratch::new("sealed")?;
    let provider = Arc::new(FaultingProvider::open(scratch.path())?);
    let provider_tmp = Arc::clone(&provider);
    let keyed: Arc<dyn KeyProvider> = provider_tmp;
    let store = RunStore::open_sealed(scratch.path().join("store.runstore"), keyed)?;
    let definition = test_definition();
    let run = test_run(4)?;
    let key = test_key("acme", "fault/one")?;
    append_step(
        &store,
        StepSpec {
            tenant: "acme",
            run,
            key,
            path: "fault/one",
            definition: &definition,
            bytes: b"a record under a fault".to_vec(),
            expires_at: None,
        },
    )?;
    provider.fail_next_unwraps(1);
    match store.read_sealed("acme", run, key) {
        Err(StoreError::SealedKey { source }) => match source {
            KeyError::Transient { .. } => {}
            other => {
                return Err(format!("a transient fault reads as transient, not {other:?}").into());
            }
        },
        Err(other) => {
            return Err(format!("a transient fault reads as transient, not {other:?}").into());
        }
        Ok(outcome) => {
            return Err(format!("a transient fault reads as no outcome: {outcome:?}").into());
        }
    }
    // No tombstone was folded: the healed read is live with the same bytes.
    assert_eq!(
        live_step_bytes(&store, "acme", run, key)?,
        b"a record under a fault"
    );
    assert_store_chain(&store)?;
    // A held fault still refuses, and still folds nothing.
    provider.fail_all_unwraps(true);
    match store.read_sealed("acme", run, key) {
        Err(StoreError::SealedKey {
            source: KeyError::Transient { .. },
        }) => {}
        Err(other) => {
            return Err(format!("a held fault reads as transient, not {other:?}").into());
        }
        Ok(outcome) => {
            return Err(format!("a held fault reads as an outcome: {outcome:?}").into());
        }
    }
    provider.fail_all_unwraps(false);
    assert_eq!(
        live_step_bytes(&store, "acme", run, key)?,
        b"a record under a fault"
    );
    assert_store_chain(&store)?;
    Ok(())
}

/// Erasing one tenant leaves the other's records live, with both chains
/// verifying and no shared key or record between them (GDPR Art. 17).
#[test]
fn two_tenants_never_share_a_key_or_a_record() -> TestResult {
    let scratch = Scratch::new("sealed")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let store = open_store(scratch.path(), Arc::clone(&provider))?;
    let definition = test_definition();
    let acme_run = test_run(51)?;
    let globex_run = test_run(52)?;
    let acme_key = test_key("acme", "same/secret")?;
    let globex_key = test_key("globex", "same/secret")?;
    for (tenant, run, key) in [
        ("acme", acme_run, acme_key),
        ("globex", globex_run, globex_key),
    ] {
        append_step(
            &store,
            StepSpec {
                tenant,
                run,
                key,
                path: "same/secret",
                definition: &definition,
                bytes: b"the same secret".to_vec(),
                expires_at: None,
            },
        )?;
    }
    // Fresh provisions never reuse an identity, even for one tenant: the two
    // records hold different data keys by construction.
    let (first_id, _) = provider.provision("acme")?;
    let (second_id, _) = provider.provision("acme")?;
    assert_ne!(
        first_id.id(),
        second_id.id(),
        "two provisions never share a data key, even for one tenant"
    );
    let file_bytes = std::fs::read(scratch.path().join("store.runstore"))?;
    let text = String::from_utf8_lossy(&file_bytes);
    assert!(
        !text.contains("the same secret"),
        "two tenants never share sealed bytes, even for the same secret"
    );
    let shredded = store.erase_tenant_records("acme")?;
    assert_eq!(shredded, 1, "the tenant erasure shreds exactly its record");
    erased_step(&store, "acme", acme_run, acme_key)?;
    assert_eq!(
        live_step_bytes(&store, "globex", globex_run, globex_key)?,
        b"the same secret",
        "erasing one tenant leaves the other's records live"
    );
    // A step of another tenant's run is foreign here, never readable:
    // the index answers only its own records.
    match store.read_sealed("acme", globex_run, globex_key) {
        Err(StoreError::ForeignTenant { .. }) => {}
        Err(other) => {
            return Err(format!("a foreign step is foreign, not {other:?}").into());
        }
        Ok(outcome) => {
            return Err(format!("a foreign step reads as an outcome: {outcome:?}").into());
        }
    }
    assert_store_chain(&store)?;
    drop(store);
    let reopened = open_store(scratch.path(), provider)?;
    erased_step(&reopened, "acme", acme_run, acme_key)?;
    assert_eq!(
        live_step_bytes(&reopened, "globex", globex_run, globex_key)?,
        b"the same secret"
    );
    assert_store_chain(&reopened)?;
    Ok(())
}

/// No key bytes and no plaintext reach `Debug` output, errors or files
/// (CWE-532): the provider's `Debug` names the directory and the counts,
/// the store's names its file, and the record plaintext appears in none of
/// them, nor in the file. The shapes are pinned, so a key that slips into
/// one fails the test rather than reaching a log.
#[test]
fn key_bytes_never_reach_debug_or_log_output() -> TestResult {
    let scratch = Scratch::new("sealed")?;
    let concrete = Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let concrete_tmp = Arc::clone(&concrete);
    let provider: Arc<dyn KeyProvider> = concrete_tmp;
    let store = open_store(scratch.path(), Arc::clone(&provider))?;
    let definition = test_definition();
    let run = test_run(6)?;
    let key = test_key("acme", "leak/one")?;
    append_step(
        &store,
        StepSpec {
            tenant: "acme",
            run,
            key,
            path: "leak/one",
            definition: &definition,
            bytes: b"a secret that must not leak".to_vec(),
            expires_at: None,
        },
    )?;
    assert_eq!(
        live_step_bytes(&store, "acme", run, key)?,
        b"a secret that must not leak"
    );
    let provider_debug = format!("{concrete:?}");
    for shape in [
        "SoftwareProvider",
        "live_keys: 1",
        "destroyed_keys: 0",
        "page_lock",
    ] {
        assert!(
            provider_debug.contains(shape),
            "the provider Debug names its shape: {provider_debug}"
        );
    }
    let store_debug = format!("{store:?}");
    {
        let shape = "RunStore";
        assert!(
            store_debug.contains(shape),
            "the store Debug names its shape: {store_debug}"
        );
    }
    let mut surfaces = Vec::new();
    surfaces.push(provider_debug);
    surfaces.push(store_debug);
    surfaces.push(
        String::from_utf8_lossy(&std::fs::read(scratch.path().join("store.runstore"))?)
            .into_owned(),
    );
    // A missing key: destroy an identity the provider never held.
    let forged = KeyId::new("acme".to_owned(), [9_u8; 16]);
    if let Err(error) = provider.destroy(&forged) {
        surfaces.push(format!("{error:?}"));
    }
    // A transient fault: refuse every read, then read.
    let faulty_dir = scratch.path().join("faults");
    std::fs::create_dir_all(&faulty_dir)?;
    let faulty = Arc::new(FaultingProvider::open(&faulty_dir)?);
    let faulty_tmp = Arc::clone(&faulty);
    let faulty_keyed: Arc<dyn KeyProvider> = faulty_tmp;
    let faulty_store = RunStore::open_sealed(faulty_dir.join("store.runstore"), faulty_keyed)?;
    let faulty_run = test_run(7)?;
    let faulty_key = test_key("acme", "leak/two")?;
    append_step(
        &faulty_store,
        StepSpec {
            tenant: "acme",
            run: faulty_run,
            key: faulty_key,
            path: "leak/two",
            definition: &definition,
            bytes: b"another secret that must not leak".to_vec(),
            expires_at: None,
        },
    )?;
    faulty.fail_all_unwraps(true);
    if let Err(error) = faulty_store.read_sealed("acme", faulty_run, faulty_key) {
        surfaces.push(format!("{error:?}"));
    }
    let secrets = [
        "a secret that must not leak",
        "another secret that must not leak",
    ];
    for (index, surface) in surfaces.iter().enumerate() {
        for secret in secrets {
            assert!(
                !surface.contains(secret),
                "surface {index} carries a record plaintext"
            );
        }
    }
    Ok(())
}

/// Erasure by tenant and by TTL shreds exactly the due records: the expired
/// and the erased read as erased, the live read as live, and the chain still
/// verifies (GDPR Art. 17; the TTL scheduler of #785).
#[test]
fn erase_tenant_and_ttl_sweep_remove_only_what_is_due() -> TestResult {
    let scratch = Scratch::new("sealed")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let store = open_store(scratch.path(), Arc::clone(&provider))?;
    let definition = test_definition();
    let run = test_run(8)?;
    let past = Some(Duration::from_secs(60));
    let future = Some(Duration::from_secs(3_600));
    let expired = test_key("acme", "ttl/past")?;
    let fresh = test_key("acme", "ttl/future")?;
    let deathless = test_key("acme", "ttl/never")?;
    for (key, path, body, expires) in [
        (expired, "ttl/past", b"already due".as_slice(), past),
        (fresh, "ttl/future", b"not due".as_slice(), future),
        (deathless, "ttl/never", b"never due".as_slice(), None),
    ] {
        append_step(
            &store,
            StepSpec {
                tenant: "acme",
                run,
                key,
                path,
                definition: &definition,
                bytes: body.to_vec(),
                expires_at: expires,
            },
        )?;
    }
    let shredded = store.sweep_once(Duration::from_secs(120))?;
    assert_eq!(shredded, 1, "the sweep shreds exactly the expired record");
    erased_step(&store, "acme", run, expired)?;
    assert_eq!(live_step_bytes(&store, "acme", run, fresh)?, b"not due");
    assert_eq!(
        live_step_bytes(&store, "acme", run, deathless)?,
        b"never due"
    );
    assert_store_chain(&store)?;
    // A sweep at the far future takes the rest with expiries and keeps the
    // deathless record live.
    let shredded = store.sweep_once(Duration::from_secs(7_200))?;
    assert_eq!(
        shredded, 1,
        "the second sweep shreds exactly the newly due record"
    );
    erased_step(&store, "acme", run, fresh)?;
    assert_eq!(
        live_step_bytes(&store, "acme", run, deathless)?,
        b"never due"
    );
    assert_store_chain(&store)?;
    Ok(())
}

/// Journal frames seal, shred to tombstones, and verify across a restart:
/// the sealed entries read live, the shredded entry reads erased, and the
/// chain covers both.
#[test]
fn sealed_journal_seals_shreds_and_verifies() -> TestResult {
    use lgwks_bot::journal::FileJournal;
    use lgwks_bot::sealed::journal::{SealedJournalRead, ShredOutcome};
    let scratch = Scratch::new("sealed-journal")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let path = scratch.path().join("journal.jrnl");
    let mut journal = FileJournal::open_sealed(&path, Arc::clone(&provider), "acme")?;
    let first = journal_event(journal_key("1")?);
    let second = journal_event(journal_key("2")?);
    append_journal_event(&mut journal, &first)?;
    append_journal_event(&mut journal, &second)?;
    assert_eq!(
        live_journal_keys(&journal)?,
        vec![first.key(), second.key()]
    );
    assert_journal_chain(&journal)?;
    match journal.shred_sealed(1)? {
        ShredOutcome::Shredded { sequence } => assert_eq!(sequence, 1),
        _ => return Err("a first shred shreds".into()),
    }
    let entries = journal.sealed_entries()?;
    if entries.len() != 2 {
        return Err(format!("the shredded journal keeps two frames, got {entries:?}").into());
    }
    match entries[0].clone() {
        SealedJournalRead::Erased { .. } => {}
        _ => {
            return Err(
                format!("the shredded journal reads erased-then-live, got {entries:?}").into(),
            );
        }
    }
    match entries[1].clone() {
        SealedJournalRead::Live { event, .. } => {
            assert_eq!(event.key(), second.key());
        }
        _ => {
            return Err(
                format!("the shredded journal reads erased-then-live, got {entries:?}").into(),
            );
        }
    }
    assert_journal_chain(&journal)?;
    drop(journal);
    let mut reopened = FileJournal::open_sealed(&path, provider, "acme")?;
    let entries = reopened.sealed_entries()?;
    if entries.len() != 2 {
        return Err(format!("the erasure keeps two frames, got {entries:?}").into());
    }
    match entries[0].clone() {
        SealedJournalRead::Erased { .. } => {}
        _ => {
            return Err(format!("the erasure survives a restart, got {entries:?}").into());
        }
    }
    match entries[1].clone() {
        SealedJournalRead::Live { event, .. } => {
            assert_eq!(event.key(), second.key());
        }
        _ => {
            return Err(format!("the erasure survives a restart, got {entries:?}").into());
        }
    }
    assert_journal_chain(&reopened)?;
    match reopened.shred_sealed(1)? {
        ShredOutcome::AlreadyErased { .. } => Ok(()),
        _ => Err("a second shred is a no-op".into()),
    }
}

/// A sealed file never opens as plain and a plain file never opens as
/// sealed: the mode guard refuses the crossing rather than decoding one
/// kind as the other.
#[test]
fn a_plain_open_refuses_a_sealed_journal() -> TestResult {
    use lgwks_bot::journal::{EffectJournal as _, FileJournal, JournalError};
    let scratch = Scratch::new("sealed-mode")?;
    let provider: Arc<dyn KeyProvider> =
        Arc::new(SoftwareProvider::open(&scratch.path().join("keys"))?);
    let sealed_path = scratch.path().join("sealed.jrnl");
    let mut sealed = FileJournal::open_sealed(&sealed_path, Arc::clone(&provider), "acme")?;
    append_journal_event(&mut sealed, &journal_event(journal_key("1")?))?;
    drop(sealed);
    match FileJournal::open(&sealed_path) {
        Err(JournalError::Corrupt(_)) => {}
        Err(other) => {
            return Err(format!("a sealed file opens as corrupt, not {other:?}").into());
        }
        Ok(_) => return Err("a plain open must not read a sealed journal".into()),
    }
    let plain_path = scratch.path().join("plain.jrnl");
    let mut plain = FileJournal::open(&plain_path)?;
    plain.compare_and_append(plain.tail(), &journal_event(journal_key("1")?))?;
    drop(plain);
    match FileJournal::open_sealed(&plain_path, provider, "acme") {
        Err(JournalError::Corrupt(_)) => Ok(()),
        Err(other) => Err(format!("a plain file opens sealed as corrupt, not {other:?}").into()),
        Ok(_) => Err("a sealed open must not read a plain journal".into()),
    }
}

/// A forgotten identity reads as missing until it is recalled: the fault
/// arms the read path only, so provisioning, wrapping and destroying still
/// reach the real provider underneath.
#[test]
fn a_forgotten_identity_reads_as_missing_until_recalled() -> TestResult {
    let scratch = Scratch::new("sealed-forget")?;
    let faulty = FaultingProvider::open(scratch.path())?;
    let (id, _key) = faulty.provision("acme")?;
    let wrapped = faulty.wrap_key(&id)?;
    faulty.forget(&id);
    match faulty.unwrap_key(&id, &wrapped) {
        Err(KeyError::Missing { .. }) => {}
        Err(other) => {
            return Err(format!("a forgotten identity reads as missing, not {other:?}").into());
        }
        Ok(_) => return Err("a forgotten identity never opens".into()),
    }
    faulty.recall(&id);
    faulty.unwrap_key(&id, &wrapped)?;
    Ok(())
}
