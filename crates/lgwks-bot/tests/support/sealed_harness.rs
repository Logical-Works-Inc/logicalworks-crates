//! Shared sealed-test harness: the faulting provider and the small drivers
//! every sealed family uses against the real stores.
//!
//! One definition of each, included by path, so `sealed.rs` and `sim_sealed.rs`
//! cannot drift: a seeded fault the simulation injects is the same fault the
//! live test names, and a read the live test asserts is the same read the
//! simulation counts.
//!
//! The drivers reach the shipped stores only through their public surface —
//! [`RunStore::open_sealed`](lgwks_bot::task::RunStore::open_sealed) and
//! [`FileJournal::open_sealed`](lgwks_bot::journal::FileJournal::open_sealed)
//! — the same constructors `estate sealed` uses, so a harness that passed
//! while the shipped store refused is impossible.

use std::collections::HashSet;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use lgwks_bot::effect::RunId;
use lgwks_bot::sealed::envelope::DataKey;
use lgwks_bot::sealed::keys::{KeyError, KeyId, KeyProvider, SoftwareProvider};
use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend};

// ── Faults ────────────────────────────────────────────────────────────────

/// A [`SoftwareProvider`] with seeded faults on the unwrap-key path.
///
/// Provisioning, wrapping, destroying and restoring delegate untouched, so a
/// fault here is exactly a provider that cannot answer *right now*: the store
/// under test still mints fresh keys, still destroys durably, and still
/// recovers across a reopen. The faults arm `unwrap_key` only — the one call
/// the read fold decides on.
#[derive(Debug)]
pub struct FaultingProvider {
    /// The real provider underneath: real keys, real files, real restarts.
    inner: SoftwareProvider,
    /// The armed faults, behind one mutex. Synchronous calls only, so no
    /// await crosses the lock.
    faults: Mutex<FaultState>,
}

/// Which reads refuse, and how.
#[derive(Debug)]
struct FaultState {
    /// How many more unwraps refuse as transient.
    transient_unwraps: u64,
    /// Whether every unwrap refuses as transient.
    transient_all: bool,
    /// Identities that read as missing while they are held here.
    forgotten: HashSet<[u8; 16]>,
}

impl FaultingProvider {
    /// Open the real provider in `dir`, with no fault armed.
    ///
    /// # Errors
    ///
    /// Whatever [`SoftwareProvider::open`] reports.
    pub fn open(dir: &Path) -> Result<Self, KeyError> {
        Ok(Self {
            inner: SoftwareProvider::open(dir)?,
            faults: Mutex::new(FaultState {
                transient_unwraps: 0,
                transient_all: false,
                forgotten: HashSet::new(),
            }),
        })
    }

    /// Refuse the next `count` unwraps as transient, then heal.
    pub fn fail_next_unwraps(&self, count: u64) {
        if let Ok(mut faults) = self.faults.lock() {
            faults.transient_unwraps = count;
        }
    }

    /// Refuse every unwrap as transient (`true`), or heal (`false`).
    pub fn fail_all_unwraps(&self, on: bool) {
        if let Ok(mut faults) = self.faults.lock() {
            faults.transient_all = on;
        }
    }

    /// Read `id` as missing until [`FaultingProvider::recall`] runs.
    pub fn forget(&self, id: &KeyId) {
        if let Ok(mut faults) = self.faults.lock() {
            faults.forgotten.insert(id.id());
        }
    }

    /// Read `id` from the real provider again.
    pub fn recall(&self, id: &KeyId) {
        if let Ok(mut faults) = self.faults.lock() {
            faults.forgotten.remove(&id.id());
        }
    }
}

impl KeyProvider for FaultingProvider {
    /// Mint through the real provider: faults never touch provisioning.
    fn provision(&self, tenant: &str) -> Result<(KeyId, DataKey), KeyError> {
        self.inner.provision(tenant)
    }

    /// Wrap through the real provider.
    fn wrap_key(&self, id: &KeyId) -> Result<Vec<u8>, KeyError> {
        self.inner.wrap_key(id)
    }

    /// Open the wrapped key, unless a fault says otherwise: a forgotten
    /// identity reads as missing, a transient arm reads as transient, and
    /// neither touches the real key.
    fn unwrap_key(&self, id: &KeyId, wrapped: &[u8]) -> Result<DataKey, KeyError> {
        /// Whether this read reaches the real provider, or which refusal it
        /// takes instead.
        enum Decision {
            /// Ask the real provider.
            Delegate,
            /// Answer with this refusal, without touching the real key.
            Refuse(KeyError),
        }
        let decision = match self.faults.lock() {
            Ok(mut faults) => {
                if faults.forgotten.contains(&id.id()) {
                    Decision::Refuse(KeyError::Missing {
                        tenant: id.tenant().to_owned(),
                        id: id.id(),
                    })
                } else if faults.transient_all {
                    Decision::Refuse(KeyError::Transient {
                        reason: "the seeded fault holds every read",
                    })
                } else if faults.transient_unwraps > 0 {
                    faults.transient_unwraps = faults.transient_unwraps.saturating_sub(1);
                    Decision::Refuse(KeyError::Transient {
                        reason: "the seeded fault holds this read",
                    })
                } else {
                    Decision::Delegate
                }
            }
            Err(_) => Decision::Refuse(KeyError::Transient {
                reason: "the fault state is unreachable",
            }),
        };
        match decision {
            Decision::Delegate => self.inner.unwrap_key(id, wrapped),
            Decision::Refuse(refusal) => Err(refusal),
        }
    }

    /// Destroy through the real provider.
    fn destroy(&self, id: &KeyId) -> Result<(), KeyError> {
        self.inner.destroy(id)
    }

    /// Restore through the real provider: a reopen replays the same keys,
    /// faults or none.
    fn note_wrapped(&self, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError> {
        self.inner.note_wrapped(id, wrapped)
    }
}

// ── Run-store drivers ─────────────────────────────────────────────────────

/// A run identity for `tag`: 32 hex chars, non-zero, so every family names
/// its runs without entropy and the same tag names the same run.
pub fn test_run(tag: u64) -> Result<RunId, Box<dyn std::error::Error>> {
    Ok(RunId::from_hex(&format!("{:032x}", tag.saturating_add(1)))?)
}

/// The step key `path` names for `tenant`: the run-less hash the store keys
/// every record by, minted through the public scope rather than spelled.
pub fn test_key(
    tenant: &str,
    path: &str,
) -> Result<lgwks_bot::script::StepKey, Box<dyn std::error::Error>> {
    use lgwks_bot::script::{Scope, Tenant};
    Ok(Scope::root(Tenant::new(tenant)?).enter(path)?.key())
}

/// The definition every sealed test records under: one name, one revision,
/// one input, one step.
pub fn test_definition() -> DefinitionIdentity {
    DefinitionIdentity::new("sealed-test", 1, lgwks_std::hash::blake3(b"sealed-test"), 1)
}

/// One sealed append as the tests name it: the tenant, run, key, path,
/// definition, bytes and expiry a step seals.
pub struct StepSpec<'a> {
    /// The tenant the run belongs to.
    pub(crate) tenant: &'a str,
    /// The run the record is for.
    pub(crate) run: RunId,
    /// The step key the record is filed under.
    pub(crate) key: lgwks_bot::script::StepKey,
    /// The step's path, for attribution.
    pub(crate) path: &'a str,
    /// The definition the value is recorded under.
    pub(crate) definition: &'a DefinitionIdentity,
    /// The archived value.
    pub(crate) bytes: Vec<u8>,
    /// When the record expires, if it does.
    pub(crate) expires_at: Option<Duration>,
}

/// Append one step, sealing it under a fresh data key.
///
/// The expiry travels with the append so the TTL tests name it; `None`
/// seals a deathless record.
pub fn append_step(
    store: &RunStore,
    spec: StepSpec<'_>,
) -> Result<lgwks_bot::script::Appended, Box<dyn std::error::Error>> {
    let StepSpec {
        tenant,
        run,
        key,
        path,
        definition,
        bytes,
        expires_at,
    } = spec;
    Ok(store.append_with_expiry(SealedAppend::new(
        tenant, run, key, path, definition, bytes, expires_at,
    ))?)
}

/// Read `key` back and require its exact bytes: the live answer, whole.
pub fn live_step_bytes(
    store: &RunStore,
    tenant: &str,
    run: RunId,
    key: lgwks_bot::script::StepKey,
) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    match store.lookup(tenant, run, key)? {
        Some(bytes) => Ok(bytes),
        None => Err("the step reads as absent, expected live".into()),
    }
}

/// Read `key` and require the tombstone: the erased answer, never bytes.
pub fn erased_step(
    store: &RunStore,
    tenant: &str,
    run: RunId,
    key: lgwks_bot::script::StepKey,
) -> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::sealed::runstore::SealedStepRead;
    match store.read_sealed(tenant, run, key)? {
        SealedStepRead::Erased { .. } => Ok(()),
        SealedStepRead::Live { .. } => Err("the step reads as live, expected erased".into()),
        _ => Err("the step reads as neither live nor erased".into()),
    }
}

/// Verify the store's chain over live and shredded records alike.
pub fn assert_store_chain(store: &RunStore) -> Result<(), Box<dyn std::error::Error>> {
    store
        .verify_sealed_chain()
        .map_err(|error| format!("the store chain does not verify: {error}").into())
}

// ── Seeded drivers ────────────────────────────────────────────────────────

/// One live record a sim holds: its step key with the exact bytes it must
/// read back while live.
pub type LivePair = (lgwks_bot::script::StepKey, Vec<u8>);

/// Deterministic payload bytes from the run's own stream: the same seed
/// seals the same bytes, so a replay compares against the same expectation.
/// A draw below 256 is its own low byte, so no conversion can fail.
pub fn seeded_bytes(rng: &mut crate::sim::Rng, len: u32) -> Vec<u8> {
    (0..len).map(|_| rng.below(256).to_le_bytes()[0]).collect()
}

/// Open a sealed run store over `dir/store.runstore` with `provider`.
pub fn open_store(
    dir: &Path,
    provider: std::sync::Arc<dyn KeyProvider>,
) -> Result<RunStore, Box<dyn std::error::Error>> {
    Ok(RunStore::open_sealed(dir.join("store.runstore"), provider)?)
}

/// Append `count` seeded records under one run, returning each key with the
/// exact bytes it must read back while live.
pub fn seeded_appends(
    sim: &mut crate::sim::Sim,
    store: &RunStore,
    tenant: &str,
    run: RunId,
    count: u32,
    max_len: u32,
    path_tag: &str,
) -> Result<Vec<LivePair>, Box<dyn std::error::Error>> {
    let definition = test_definition();
    let mut appended = Vec::new();
    for index in 0..count {
        let len = sim.rng().between(1, max_len);
        let body = seeded_bytes(sim.rng(), len);
        let key = test_key(tenant, &format!("{path_tag}/{index}"))?;
        append_step(
            store,
            StepSpec {
                tenant,
                run,
                key,
                path: &format!("{path_tag}/{index}"),
                definition: &definition,
                bytes: body.clone(),
                expires_at: None,
            },
        )?;
        appended.push((key, body));
    }
    Ok(appended)
}

/// Fail a scenario with the seed attached, so a failing band names its seed.
pub fn refuse_seeded(seed: u64, detail: String) -> Box<dyn std::error::Error> {
    format!("seed {seed}: {detail}").into()
}

/// Read `key` and require the live bytes to equal `expected`.
pub fn require_live(
    store: &RunStore,
    tenant: &str,
    run: RunId,
    key: lgwks_bot::script::StepKey,
    expected: &[u8],
    why: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let bytes =
        live_step_bytes(store, tenant, run, key).map_err(|error| format!("{why}: {error}"))?;
    assert_eq!(
        bytes, expected,
        "{why}: a live record reads its exact bytes"
    );
    Ok(())
}

// ── Journal drivers ───────────────────────────────────────────────────────

/// The fixed identities every sealed journal test shares: one run, one
/// action, one environment, one flow and one digest, so two tests fold the
/// same world under what looks like the same key.
pub mod journal_fixture {
    /// The run every fixture key belongs to.
    pub const RUN: &str = "0102030405060708090a0b0c0d0e0f10";
    /// The action every fixture key names.
    pub const ACTION: &str = "1112131415161718191a1b1c1d1e1f20";
    /// The environment every fixture key is fenced against.
    pub const ENV: &str = "2122232425262728292a2b2c2d2e2f30";
    /// The flow revision every fixture key was produced from.
    pub const FLOW_HEX: &str = "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f";
    /// The input digest every fixture key binds.
    pub const DIGEST_HEX: &str = "f0f1f2f3f4f5f6f7f8f9fafbfcfdfeffe0e1e2e3e4e5e6e7e8e9eaebecedeeef";
}

/// One fixture key for `attempt`: every field but the attempt is fixed, and a
/// test that wants a second attempt differs only in that field.
pub fn journal_key(
    attempt: &str,
) -> Result<lgwks_bot::effect::EffectKey, Box<dyn std::error::Error>> {
    use lgwks_bot::effect::{
        ActionDigest, ActionId, AttemptId, EffectIdentity, EnvironmentEpoch, EnvironmentId,
        FlowRevision, RunId,
    };
    let run = RunId::from_hex(journal_fixture::RUN)?;
    let action = ActionId::from_hex(journal_fixture::ACTION)?;
    let attempt = AttemptId::from_decimal(attempt)?;
    let flow = FlowRevision::from_tagged("blake3_256", journal_fixture::FLOW_HEX)?;
    let digest = ActionDigest::from_tagged("blake3_256", journal_fixture::DIGEST_HEX)?;
    let environment = EnvironmentId::from_hex(journal_fixture::ENV)?;
    let epoch = EnvironmentEpoch::from_decimal("1")?;
    Ok(EffectIdentity::new(run, environment, flow).key(action, attempt, digest, epoch))
}

/// One sealed journal event: the admission rung for `key`.
pub fn journal_event(key: lgwks_bot::effect::EffectKey) -> lgwks_bot::journal::EffectEvent {
    lgwks_bot::journal::EffectEvent::IntentAdmitted { key }
}

/// Seal `event` onto the journal's own tail with no expiry.
///
/// Compares-and-appends at the current tail, so the frame lands exactly where
/// a live writer would put it and a concurrent writer is refused rather than
/// interleaved. No expiry travels with it: journal frames do not ttl out,
/// only shred.
pub fn append_journal_event(
    journal: &mut lgwks_bot::journal::FileJournal,
    event: &lgwks_bot::journal::EffectEvent,
) -> Result<(), Box<dyn std::error::Error>> {
    use lgwks_bot::journal::EffectJournal as _;
    let at = journal.tail();
    journal.compare_and_append_sealed(at, event, None)?;
    Ok(())
}

/// Every live event in commit order: the attempt keys the sealed frames
/// still open under.
pub fn live_journal_keys(
    journal: &lgwks_bot::journal::FileJournal,
) -> Result<Vec<lgwks_bot::effect::EffectKey>, Box<dyn std::error::Error>> {
    use lgwks_bot::sealed::journal::SealedJournalRead;
    let mut out = Vec::new();
    for read in journal.sealed_entries()? {
        match read {
            SealedJournalRead::Live { event, .. } => out.push(event.key()),
            SealedJournalRead::Erased { .. } => {}
            _ => {}
        }
    }
    Ok(out)
}

/// Verify the journal's chain over sealed and shredded frames alike.
pub fn assert_journal_chain(
    journal: &lgwks_bot::journal::FileJournal,
) -> Result<(), Box<dyn std::error::Error>> {
    journal
        .verify_sealed_chain()
        .map_err(|error| format!("the journal chain does not verify: {error}").into())
}

/// A tenant name for `tag` and `index`: file-safe by construction.
pub fn tenant_name(tag: &str, index: u64) -> String {
    format!("{tag}-{index}")
}
