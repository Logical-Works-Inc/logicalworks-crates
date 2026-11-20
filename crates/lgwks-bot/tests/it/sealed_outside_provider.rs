//! A key provider written outside `lgwks_bot` (#1060).
//!
//! Source: issue #1060. `DataKey` had no constructor outside the crate, so no
//! crate could implement `KeyProvider`, and a host could not keep its record
//! keys in a key service, an HSM or a platform keystore. The store now hands
//! each `provision` and `unwrap_key` call one `KeyMint`, which an outside
//! provider spends on exactly one key. This file is that outside crate: the
//! integration test binary links `lgwks_bot` as any host would.
//!
//! `KeystoreProvider` is handle-based, the shape of a platform keystore: the
//! wrapped bytes it hands the store are a handle, and the keys live in its own
//! owner-only file, which a reopen reads back.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lgwks_bot::sealed::conformance;
use lgwks_bot::sealed::envelope::{DataKey, KEY_BYTES, KeyMint};
use lgwks_bot::sealed::keys::{KeyError, KeyId, KeyProvider, SoftwareProvider};

use crate::scratch::Scratch;
use crate::sealed_harness::{
    StepSpec, append_step, erased_step, live_step_bytes, open_store, test_definition, test_key,
    test_run,
};

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The longest tenant name the keystore file holds.
const TENANT_BYTES: usize = 15;

/// One keystore entry: the tenant's length (0 marks a destruction), the
/// tenant padded to [`TENANT_BYTES`], the 16-byte handle and the key.
const ENTRY_BYTES: usize = 1 + TENANT_BYTES + 16 + KEY_BYTES;

/// What the keystore holds, behind one lock.
#[derive(Default)]
struct Held {
    /// Live keys by handle, with their tenant.
    keys: HashMap<[u8; 16], (String, [u8; KEY_BYTES])>,
    /// Destroyed handles.
    destroyed: HashSet<[u8; 16]>,
    /// The next handle's counter.
    next: u64,
}

/// A handle-based keystore provider, defined outside `lgwks_bot`.
struct KeystoreProvider {
    /// The keystore file: one [`ENTRY_BYTES`] entry per mint and one per
    /// destruction, appended and synced, replayed in order on open.
    file: PathBuf,
    /// The keystore's state.
    held: Mutex<Held>,
}

/// One keystore entry's fields: the tenant's length, the padded tenant, the
/// handle and the key.
type Entry<'a> = (usize, &'a [u8], [u8; 16], [u8; KEY_BYTES]);

/// The store's view of the keystore: the same provider, as the trait object
/// the store holds.
fn shared(provider: &Arc<KeystoreProvider>) -> Arc<dyn KeyProvider> {
    Arc::<KeystoreProvider>::clone(provider)
}

/// The fields of one keystore entry.
fn split_entry(entry: &[u8; ENTRY_BYTES]) -> Result<Entry<'_>, KeyError> {
    let (len, rest) = entry.split_at(1);
    let (tenant, rest) = rest.split_at(TENANT_BYTES);
    let (handle, key) = rest.split_at(16);
    let handle: [u8; 16] = handle.try_into().map_err(|cause| {
        lgwks_std::trace::warn!(%cause, "keystore: a handle is misshapen");
        KeyError::Corrupt {
            reason: "a keystore handle is misshapen",
        }
    })?;
    let key: [u8; KEY_BYTES] = key.try_into().map_err(|cause| {
        lgwks_std::trace::warn!(%cause, "keystore: a key is misshapen");
        KeyError::Corrupt {
            reason: "a keystore key is misshapen",
        }
    })?;
    let Some(&len) = len.first() else {
        lgwks_std::trace::warn!("keystore: an entry has no length");
        return Err(KeyError::Corrupt {
            reason: "a keystore entry has no length",
        });
    };
    Ok((usize::from(len), tenant, handle, key))
}

impl KeystoreProvider {
    /// Open the keystore in `dir`, reading back what a previous open wrote.
    fn open(dir: &Path) -> Result<Self, KeyError> {
        let file = dir.join("keystore.bin");
        let mut held = Held::default();
        match std::fs::read(&file) {
            Ok(raw) => {
                let (entries, torn) = raw.as_chunks::<ENTRY_BYTES>();
                if !torn.is_empty() {
                    lgwks_std::trace::warn!(torn = torn.len(), "keystore: the file is torn");
                    return Err(KeyError::Corrupt {
                        reason: "the keystore file is torn",
                    });
                }
                for entry in entries {
                    let (len, tenant, handle, key) = split_entry(entry)?;
                    if len == 0 {
                        held.keys.remove(&handle);
                        held.destroyed.insert(handle);
                    } else {
                        let Some(name) = tenant.get(..len) else {
                            lgwks_std::trace::warn!(len, "keystore: a tenant overruns its field");
                            return Err(KeyError::Corrupt {
                                reason: "a keystore tenant overruns its field",
                            });
                        };
                        let name = String::from_utf8(name.to_vec()).map_err(|cause| {
                            lgwks_std::trace::warn!(%cause, "keystore: a tenant is not UTF-8");
                            KeyError::Corrupt {
                                reason: "a keystore tenant is not UTF-8",
                            }
                        })?;
                        held.keys.insert(handle, (name, key));
                    }
                    held.next = held.next.max(counter_of(&handle).saturating_add(1));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(cause) => {
                lgwks_std::trace::warn!(%cause, "keystore: the file could not be read");
                return Err(KeyError::Storage { cause });
            }
        }
        Ok(Self {
            file,
            held: Mutex::new(held),
        })
    }

    /// Append one entry to the keystore file, owner-only: a live key under
    /// `tenant`, or a destruction when `tenant` is empty.
    fn append(
        &self,
        tenant: &str,
        handle: &[u8; 16],
        key: &[u8; KEY_BYTES],
    ) -> Result<(), KeyError> {
        use std::io::Write as _;
        if tenant.len() > TENANT_BYTES {
            lgwks_std::trace::warn!(len = tenant.len(), "keystore: a tenant name is too long");
            return Err(KeyError::Corrupt {
                reason: "a tenant name is longer than the keystore holds",
            });
        }
        let len = u8::try_from(tenant.len()).map_err(|cause| {
            lgwks_std::trace::warn!(%cause, "keystore: a tenant length does not fit a byte");
            KeyError::Corrupt {
                reason: "a tenant name is longer than the keystore holds",
            }
        })?;
        let mut entry = Vec::with_capacity(ENTRY_BYTES);
        entry.push(len);
        let mut padded = [0_u8; TENANT_BYTES];
        padded
            .get_mut(..tenant.len())
            .ok_or(KeyError::Corrupt {
                reason: "a tenant name is longer than the keystore holds",
            })?
            .copy_from_slice(tenant.as_bytes());
        entry.extend_from_slice(&padded);
        entry.extend_from_slice(handle);
        entry.extend_from_slice(key);
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.file)?;
        file.write_all(&entry)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            std::fs::set_permissions(&self.file, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    /// The handles this keystore holds live, for the test to name.
    fn live_handles(&self) -> Result<Vec<[u8; 16]>, KeyError> {
        Ok(self.lock()?.keys.keys().copied().collect())
    }

    /// The live key `id` names for its own tenant.
    fn live(held: &Held, id: &KeyId) -> Result<[u8; KEY_BYTES], KeyError> {
        if held.destroyed.contains(&id.id()) {
            lgwks_std::trace::debug!(tenant = id.tenant(), "keystore: the key was destroyed");
            return Err(KeyError::Destroyed {
                tenant: id.tenant().to_owned(),
                id: id.id(),
            });
        }
        match held.keys.get(&id.id()) {
            Some(entry) if entry.0 == id.tenant() => Ok(entry.1),
            _ => Err(KeyError::Missing {
                tenant: id.tenant().to_owned(),
                id: id.id(),
            }),
        }
    }

    /// Lock the state, reporting a poisoned lock as transient.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Held>, KeyError> {
        self.held.lock().map_err(|cause| {
            lgwks_std::trace::warn!(%cause, "keystore: the lock was poisoned");
            KeyError::Transient {
                reason: "the keystore lock was poisoned",
            }
        })
    }
}

/// The counter a handle carries in its first eight bytes.
fn counter_of(handle: &[u8; 16]) -> u64 {
    let mut head = [0_u8; 8];
    head.copy_from_slice(&handle[..8]);
    u64::from_le_bytes(head)
}

impl KeyProvider for KeystoreProvider {
    /// Mint a fresh key for `tenant` and spend `mint` on the store's copy.
    fn provision_with(&self, tenant: &str, mint: KeyMint) -> Result<(KeyId, DataKey), KeyError> {
        let mut held = self.lock()?;
        let key = lgwks_std::random::bytes::<KEY_BYTES>().map_err(|cause| {
            lgwks_std::trace::warn!(%cause, "keystore: no entropy for a key");
            KeyError::Transient {
                reason: "no entropy for a keystore key",
            }
        })?;
        let tail = lgwks_std::random::bytes::<8>().map_err(|cause| {
            lgwks_std::trace::warn!(%cause, "keystore: no entropy for a handle");
            KeyError::Transient {
                reason: "no entropy for a keystore handle",
            }
        })?;
        let mut handle = [0_u8; 16];
        handle[..8].copy_from_slice(&held.next.to_le_bytes());
        handle[8..].copy_from_slice(&tail);
        held.next = held.next.saturating_add(1);
        if tenant.is_empty() {
            lgwks_std::trace::warn!("keystore: a provision named no tenant");
            return Err(KeyError::Corrupt {
                reason: "the keystore names no tenant with an empty name",
            });
        }
        self.append(tenant, &handle, &key)?;
        held.keys.insert(handle, (tenant.to_owned(), key));
        Ok((KeyId::new(tenant.to_owned(), handle), mint.key(key)))
    }

    /// The wrapped bytes are the handle: the key never leaves the keystore.
    fn wrap_key(&self, id: &KeyId) -> Result<Vec<u8>, KeyError> {
        let held = self.lock()?;
        Self::live(&held, id)?;
        Ok(id.id().to_vec())
    }

    /// Look the handle up and spend `mint` on the live key.
    fn unwrap_with(&self, id: &KeyId, wrapped: &[u8], mint: KeyMint) -> Result<DataKey, KeyError> {
        let held = self.lock()?;
        let key = Self::live(&held, id)?;
        if wrapped != id.id() {
            lgwks_std::trace::warn!(
                tenant = id.tenant(),
                "keystore: an unwrap handle names another key"
            );
            return Err(KeyError::Corrupt {
                reason: "the handle does not name this key",
            });
        }
        Ok(mint.key(key))
    }

    /// Remove the key and record the destruction in the keystore file.
    fn destroy(&self, id: &KeyId) -> Result<(), KeyError> {
        let mut held = self.lock()?;
        if held.destroyed.contains(&id.id()) {
            return Ok(());
        }
        Self::live(&held, id)?;
        self.append("", &id.id(), &[0; KEY_BYTES])?;
        held.keys.remove(&id.id());
        held.destroyed.insert(id.id());
        Ok(())
    }

    /// The keystore reads its own file on open, so a note only checks that
    /// the handle is one this keystore minted.
    fn note_wrapped(&self, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError> {
        let held = self.lock()?;
        if wrapped != id.id() {
            lgwks_std::trace::warn!(
                tenant = id.tenant(),
                "keystore: a noted handle names another key"
            );
            return Err(KeyError::Corrupt {
                reason: "the handle does not name this key",
            });
        }
        if held.destroyed.contains(&id.id()) || held.keys.contains_key(&id.id()) {
            return Ok(());
        }
        Err(KeyError::Corrupt {
            reason: "the keystore never minted this handle",
        })
    }
}

/// The kit, run against a provider defined outside the crate, passes every
/// seed with the same trace hash as the software provider.
#[test]
fn an_outside_provider_passes_the_conformance_kit() -> TestResult {
    for seed in 1_u64..=64 {
        let outside = Scratch::new("outside-kit")?;
        let dir = outside.path().to_path_buf();
        let mut open_outside = || -> Result<Arc<dyn KeyProvider>, KeyError> {
            Ok(Arc::new(KeystoreProvider::open(&dir)?))
        };
        let theirs = conformance::check(&mut open_outside, seed);

        let software = Scratch::new("software-kit")?;
        let soft_dir = software.path().to_path_buf();
        let mut open_software = || -> Result<Arc<dyn KeyProvider>, KeyError> {
            Ok(Arc::new(SoftwareProvider::open(&soft_dir)?))
        };
        let ours = conformance::check(&mut open_software, seed);

        assert!(
            ours.passed(),
            "seed {seed}: SoftwareProvider: {:?}",
            ours.violations()
        );
        assert!(
            theirs.passed(),
            "seed {seed}: KeystoreProvider: {:?}",
            theirs.violations()
        );
        assert_eq!(
            theirs.trace_hash(),
            ours.trace_hash(),
            "seed {seed}: an outside provider runs the same trace as the software one"
        );
    }
    Ok(())
}

/// A tenant cannot destroy or read a key another tenant's record sealed under,
/// and the owner's record stays live.
#[test]
fn a_foreign_tenant_key_id_is_refused_by_an_outside_provider() -> TestResult {
    let scratch = Scratch::new("outside-foreign")?;
    let provider = Arc::new(KeystoreProvider::open(scratch.path())?);
    let store = open_store(scratch.path(), shared(&provider))?;
    let run = test_run(11)?;
    let key = test_key("acme", "outside/foreign")?;
    let definition = test_definition();
    append_step(
        &store,
        StepSpec {
            tenant: "acme",
            run,
            key,
            path: "outside/foreign",
            definition: &definition,
            bytes: b"acme's record".to_vec(),
            expires_at: None,
        },
    )?;
    let handles = provider.live_handles()?;
    assert_eq!(handles.len(), 1, "one append minted one key");
    for handle in handles {
        let forged = KeyId::new("globex".to_owned(), handle);
        match provider.destroy(&forged) {
            Err(KeyError::Missing { tenant, .. }) => assert_eq!(tenant, "globex"),
            other => return Err(format!("globex destroyed acme's key: {other:?}").into()),
        }
    }
    let foreign = match store.lookup("globex", run, key) {
        Ok(found) => found,
        Err(error) => {
            lgwks_std::trace::debug!(%error, "globex's read of acme's record was refused");
            None
        }
    };
    assert_ne!(
        foreign,
        Some(b"acme's record".to_vec()),
        "globex never reads acme's bytes"
    );
    assert_eq!(live_step_bytes(&store, "acme", run, key)?, b"acme's record");
    Ok(())
}

/// Dropping the provider and the store and opening both again keeps live keys
/// live and destroyed keys destroyed.
#[test]
fn an_outside_provider_restores_keys_after_a_reopen() -> TestResult {
    let scratch = Scratch::new("outside-reopen")?;
    let run = test_run(12)?;
    let definition = test_definition();
    let mut keys = Vec::new();
    {
        let provider = Arc::new(KeystoreProvider::open(scratch.path())?);
        let store = open_store(scratch.path(), shared(&provider))?;
        for index in 0..3_u8 {
            let path = format!("outside/reopen/{index}");
            let key = test_key("acme", &path)?;
            append_step(
                &store,
                StepSpec {
                    tenant: "acme",
                    run,
                    key,
                    path: &path,
                    definition: &definition,
                    bytes: vec![index; 8],
                    expires_at: None,
                },
            )?;
            let mut handles = provider.live_handles()?;
            handles.sort_unstable_by_key(counter_of);
            let Some(&newest) = handles.last() else {
                return Err("an append minted no key".into());
            };
            keys.push((key, newest));
        }
        let Some(&(_, first)) = keys.first() else {
            return Err("no record was appended".into());
        };
        provider.destroy(&KeyId::new("acme".to_owned(), first))?;
        drop(store);
        drop(provider);
    }
    let provider = Arc::new(KeystoreProvider::open(scratch.path())?);
    let store = open_store(scratch.path(), shared(&provider))?;
    for (index, &(key, _)) in keys.iter().enumerate() {
        if index == 0 {
            erased_step(&store, "acme", run, key)?;
        } else {
            let byte = u8::try_from(index)?;
            assert_eq!(live_step_bytes(&store, "acme", run, key)?, vec![byte; 8]);
        }
    }
    match provider.destroy(&KeyId::new("acme".to_owned(), [0xee; 16])) {
        Err(KeyError::Missing { .. }) => Ok(()),
        other => Err(format!("a never-minted handle was not missing: {other:?}").into()),
    }
}

/// Eight threads mint 1,000 keys each through one shared outside provider:
/// every identity is distinct and stays its own tenant's, and the mint token
/// adds no bytes to a call.
#[test]
fn an_outside_provider_mints_distinct_keys_across_threads() -> TestResult {
    assert_eq!(
        std::mem::size_of::<KeyMint>(),
        0,
        "the mint token is zero-sized"
    );
    let scratch = Scratch::new("outside-threads")?;
    let provider: Arc<dyn KeyProvider> = Arc::new(KeystoreProvider::open(scratch.path())?);
    let minted: Vec<Result<Vec<KeyId>, KeyError>> = std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(8);
        for thread in 0..8_u8 {
            let provider = Arc::clone(&provider);
            handles.push(scope.spawn(move || {
                conformance::mint_ids(provider.as_ref(), &format!("tenant-{thread}"), 1_000)
            }));
        }
        handles
            .into_iter()
            .map(|handle| match handle.join() {
                Ok(minted) => minted,
                Err(_) => Err(KeyError::Transient {
                    reason: "a minting thread panicked",
                }),
            })
            .collect()
    });
    let mut seen = HashSet::new();
    for (thread, ids) in minted.into_iter().enumerate() {
        let ids = ids?;
        assert_eq!(ids.len(), 1_000, "thread {thread} minted its count");
        for id in ids {
            assert_eq!(id.tenant(), format!("tenant-{thread}"));
            assert!(seen.insert(id.id()), "an identity was minted twice");
        }
    }
    assert_eq!(seen.len(), 8_000);
    Ok(())
}
