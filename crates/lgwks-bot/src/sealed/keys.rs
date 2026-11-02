//! Wrapped data keys: the [`KeyProvider`] port, its typed refusals, and the
//! software provider that ships behind it.
//!
//! A sealed record never shares its data key: [`KeyProvider::provision`] mints
//! a fresh 32-byte key per record, and the provider wraps each key for storage
//! beside its ciphertext. Shredding is [`KeyProvider::destroy`]: the key is
//! removed and zeroized, and its identity is appended to an owner-only
//! `destroyed.log`, so a restart still refuses to resurrect it. Reprovisioning
//! a destroyed identity is refused the same way — provision mints fresh
//! identities and never reuses one.
//!
//! # Page locking
//!
//! Keys are zeroized on drop and never appear in `Debug` output or logs, and
//! key files are owner-only. They are not page-locked: the estate has no OS
//! page-lock seam, and admitting an FFI edge for `mlock` alone would trade the
//! workspace's `unsafe` ban for one call. [`SoftwareProvider::page_lock_report`]
//! reports exactly that, so a caller choosing this provider knows what it
//! does and does not promise.
//!
//! [`KeyProvider`]: crate::sealed::keys::KeyProvider
//! [`KeyProvider::provision`]: crate::sealed::keys::KeyProvider::provision
//! [`KeyProvider::destroy`]: crate::sealed::keys::KeyProvider::destroy
//! [`SoftwareProvider::page_lock_report`]: crate::sealed::keys::SoftwareProvider::page_lock_report

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use super::envelope::{DataKey, KEY_BYTES, NONCE_BYTES, TAG_BYTES, open, seal};

/// Restrict `path` to its owner: files `0o600`, directories `0o700`.
///
/// One definition for both sealed files and the provider's key files, so the
/// two cannot drift. Unix only: ownership is a unix concept, and on other
/// targets this is a documented no-op rather than a refused open — the
/// owner-only claim the suite holds is the unix-gated one.
pub(crate) fn restrict_file(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// Restrict the directory `path` to its owner (`0o700` on unix, a no-op
/// elsewhere, for the same reason as [`restrict_file`]).
pub(crate) fn restrict_dir(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

/// What the page locker reports, because there is no page locker.
///
/// Returned by [`SoftwareProvider::page_lock_report`]: a static sentence a
/// caller can print, not a boolean a reader must interpret.
pub(crate) const PAGE_LOCK_REPORT: &str = "not held: the estate has no page-lock seam, so keys may page; they are zeroized on drop, key files are owner-only, and no key bytes reach Debug output or logs";

/// One data key's identity: the tenant it was minted for and 16 random bytes.
///
/// The identity travels in the sealed frame and in the chain head, which is
/// what binds a ciphertext to its key without ever holding the key itself.
/// It carries no secret, so it prints whole; the secret lives in
/// [`DataKey`], which does not.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct KeyId {
    /// The tenant the key was minted for. Part of the identity so one
    /// provider serving many tenants cannot unwrap across them.
    pub(crate) tenant: String,
    /// The key's 16 random bytes. Unique per provision, never reused after a
    /// destruction.
    pub(crate) id: [u8; 16],
}

impl KeyId {
    /// Name an identity the provider may or may not hold.
    ///
    /// The one door for callers outside the crate: the fields stay
    /// crate-internal so a tenant label can never be rewritten past the
    /// provider that minted it, while tests and journeys can still name a
    /// forged identity to prove it reads as missing.
    #[must_use]
    pub fn new(tenant: String, id: [u8; 16]) -> Self {
        Self { tenant, id }
    }

    /// The tenant the key was minted for.
    #[must_use]
    pub fn tenant(&self) -> &str {
        self.tenant.as_str()
    }

    /// The key's 16 random bytes.
    #[must_use]
    pub const fn id(&self) -> [u8; 16] {
        self.id
    }
}

/// Why a key operation refused.
///
/// Destroyed, missing and transient are three different facts — erased,
/// never-held, and try-again — and the read fold answers each differently, so
/// they are three arms rather than one error with a message.
#[derive(Debug)]
#[non_exhaustive]
pub enum KeyError {
    /// The key was destroyed: the record it sealed reads as erased, never as
    /// an error. Returned rather than folded here because the fold, not the
    /// provider, decides what a read shows.
    Destroyed {
        /// The tenant the destroyed key belonged to.
        tenant: String,
        /// The destroyed key's identity.
        id: [u8; 16],
    },
    /// The provider holds no key under this identity: it was never
    /// provisioned, or it belongs to another tenant.
    Missing {
        /// The tenant that was asked.
        tenant: String,
        /// The identity that was asked for.
        id: [u8; 16],
    },
    /// The provider could not answer right now, and nothing changed: the key
    /// is neither returned nor destroyed, so the caller retries rather than
    /// folding the record either way.
    Transient {
        /// What could not be reached, without key bytes.
        reason: &'static str,
    },
    /// The device holding the keys refused.
    Storage {
        /// What the device said.
        cause: std::io::Error,
    },
    /// The wrapped key bytes do not decode: not a missing key, a damaged one.
    Corrupt {
        /// What failed to decode, without key bytes.
        reason: &'static str,
    },
}

impl core::fmt::Display for KeyError {
    /// The refusal, naming the identity and never the key.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Destroyed { ref tenant, ref id } => {
                write!(
                    formatter,
                    "key {} of tenant {tenant:?} was destroyed",
                    hex_of(id)
                )
            }
            Self::Missing { ref tenant, ref id } => {
                write!(formatter, "no key {} for tenant {tenant:?}", hex_of(id))
            }
            Self::Transient { reason } => {
                write!(
                    formatter,
                    "the key provider is unavailable ({reason}); nothing changed, retry"
                )
            }
            Self::Storage { ref cause } => {
                write!(formatter, "the key store's device refused: {cause}")
            }
            Self::Corrupt { reason } => {
                write!(formatter, "the wrapped key does not decode ({reason})")
            }
        }
    }
}

impl std::error::Error for KeyError {
    /// The device's own error, when the device is what refused.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Storage { ref cause } => Some(cause),
            Self::Destroyed { .. }
            | Self::Missing { .. }
            | Self::Transient { .. }
            | Self::Corrupt { .. } => None,
        }
    }
}

impl From<std::io::Error> for KeyError {
    /// A device refusal is a storage refusal, carrying the device's own error.
    fn from(cause: std::io::Error) -> Self {
        Self::Storage { cause }
    }
}

/// The hex spelling of a key identity, for refusals and logs. Identities are
/// not secret; keys are, and this function only ever sees the former.
fn hex_of(id: &[u8; 16]) -> String {
    id.iter().map(|byte| format!("{byte:02x}")).collect()
}

impl core::fmt::Display for KeyId {
    /// The tenant and the hex identity: everything a log needs to match a
    /// tombstone to its destruction, and nothing secret.
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(formatter, "{}:{}", self.tenant, hex_of(&self.id))
    }
}

/// Mint, wrap, open and destroy per-record data keys.
///
/// One fresh key per record, never shared across records or tenants: the
/// store calls [`KeyProvider::provision`] once per append and never reuses
/// the answer. `provision` hands the fresh key to the caller once — the
/// caller seals under it, wraps it for storage, and drops it — while the
/// provider keeps its own copy for later `unwrap_key` calls. Object-safe and
/// shareable, because the sealed store holds it behind an `Arc` and the
/// background sweeper reaches across threads through it.
pub trait KeyProvider: Send + Sync {
    /// Mint a fresh data key for `tenant`, returning its identity and the key
    /// itself for the one seal it was minted for.
    ///
    /// # Errors
    ///
    /// [`KeyError::Transient`] when entropy is unavailable, [`KeyError::Storage`]
    /// when the provider's device refused.
    fn provision(&self, tenant: &str) -> Result<(KeyId, DataKey), KeyError>;

    /// Export the key under `id`, sealed under the provider's master: the
    /// bytes the sealed frame stores beside its ciphertext.
    ///
    /// # Errors
    ///
    /// [`KeyError::Destroyed`] when the key was destroyed, [`KeyError::Missing`]
    /// when the provider holds no such key, [`KeyError::Transient`] when the
    /// seal step could not run.
    fn wrap_key(&self, id: &KeyId) -> Result<Vec<u8>, KeyError>;

    /// Open `wrapped` for `id`, returning the live data key.
    ///
    /// # Errors
    ///
    /// [`KeyError::Destroyed`] when the key was destroyed — the caller folds
    /// this to a tombstone, never to bytes — [`KeyError::Missing`] when the
    /// provider holds no such key, [`KeyError::Corrupt`] when the wrapped
    /// bytes do not decode, [`KeyError::Transient`] when the provider could
    /// not answer.
    fn unwrap_key(&self, id: &KeyId, wrapped: &[u8]) -> Result<DataKey, KeyError>;

    /// Destroy the key under `id`: remove it, zeroize it, and record the
    /// destruction durably, so a restart still refuses to resurrect it.
    /// Idempotent: destroying a destroyed key is a no-op.
    ///
    /// # Errors
    ///
    /// [`KeyError::Missing`] when the provider never held the key,
    /// [`KeyError::Storage`] when the destruction could not be recorded — in
    /// which case the key is still live and the caller must treat the record
    /// as live too.
    fn destroy(&self, id: &KeyId) -> Result<(), KeyError>;

    /// Restore the live key `wrapped` names under `id`, after a restart.
    ///
    /// Live keys are memory; the wrapped bytes beside each sealed frame are
    /// durable. The sealed store replays its frames on open and hands every
    /// live record back through here, so a reopened provider answers the
    /// same keys without ever seeing them twice. A destroyed identity is a
    /// no-op success — the destruction, not the note, is authoritative — and
    /// an already-noted identity keeps its first key.
    ///
    /// # Errors
    ///
    /// [`KeyError::Corrupt`] when the wrapped bytes do not open under this
    /// provider: the frame is not this provider's to read, and replaying it
    /// as live would be a lie.
    fn note_wrapped(&self, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError>;
}

/// How many bytes a wrapped key occupies: the seal nonce and the sealed
/// 32-byte data key with its tag.
const WRAPPED_BYTES: usize = NONCE_BYTES + KEY_BYTES + TAG_BYTES;

/// The master key file's name inside the provider directory.
const MASTER_FILE: &str = "master.key";

/// The destruction log's name inside the provider directory: one 16-byte
/// identity per destroyed key, appended and synced per destruction.
const DESTROYED_FILE: &str = "destroyed.log";

/// The software key provider: an in-memory map of data keys behind a
/// file-backed master key and an append-only destruction log.
///
/// The master key is generated once, stored owner-only, and held in a
/// zeroizing [`DataKey`]; wrapping seals a data key under it with a nonce
/// derived from the key identity, so every wrap under one master still uses a
/// fresh nonce. `Debug` names the directory and the counts, never the keys.
pub struct SoftwareProvider {
    /// Where `master.key` and `destroyed.log` live.
    dir: PathBuf,
    /// The live state, behind one mutex. Every method takes it once and holds
    /// no await under it — these are synchronous calls, so there is no await
    /// to hold it across.
    state: std::sync::Mutex<ProviderState>,
}

/// The live keys and destructions one provider holds.
struct ProviderState {
    /// The master every wrap seals under. Zeroized on drop with the rest.
    master: DataKey,
    /// Live data keys by identity, with the tenant each was minted for.
    keys: HashMap<[u8; 16], (String, DataKey)>,
    /// Destroyed identities. Checked before the live map, so a destroyed key
    /// reads as destroyed even if its wrapped bytes still decode.
    destroyed: HashSet<[u8; 16]>,
    /// The destruction log, kept open for appends. Every destruction is
    /// synced before the in-memory key is removed, so a synced destruction
    /// survives a crash and an unsynced one leaves the key live.
    log: File,
}

impl ProviderState {
    /// The live data key `id` names for its own tenant: a destroyed key is
    /// [`KeyError::Destroyed`], and a key never held or held for another
    /// tenant is [`KeyError::Missing`], checked in that order.
    fn live_key(&self, id: &KeyId) -> Result<&DataKey, KeyError> {
        let refusal = if self.destroyed.contains(&id.id) {
            KeyError::Destroyed {
                tenant: id.tenant.clone(),
                id: id.id,
            }
        } else {
            match self.keys.get(&id.id) {
                Some(held) if held.0 == id.tenant => return Ok(&held.1),
                _ => KeyError::Missing {
                    tenant: id.tenant.clone(),
                    id: id.id,
                },
            }
        };
        lgwks_std::trace::debug!(error = ?refusal, "provider: the key is not live for this tenant");
        Err(refusal)
    }

    /// Open `wrapped` under the master, returning the data key it seals.
    ///
    /// The one door `unwrap` and `note_wrapped` open through, so the shape
    /// check, the nonce split and the authentication cannot drift between a
    /// read and a replay: either both accept the bytes or neither does.
    fn open_wrapped(&self, wrapped: &[u8]) -> Result<DataKey, KeyError> {
        if wrapped.len() != WRAPPED_BYTES {
            lgwks_std::trace::debug!(
                len = wrapped.len(),
                "provider: the wrapped key is not one wrap"
            );
            return Err(KeyError::Corrupt {
                reason: "the wrapped key is not one wrap",
            });
        }
        let (nonce, sealed) = wrapped.split_at(NONCE_BYTES);
        let Ok(nonce) = <[u8; NONCE_BYTES]>::try_from(nonce) else {
            lgwks_std::trace::debug!("provider: the wrap nonce is misshapen");
            return Err(KeyError::Corrupt {
                reason: "the wrap nonce is misshapen",
            });
        };
        let opened = open(&self.master, &nonce, sealed).map_err(|error| {
            lgwks_std::trace::debug!(error = ?error, "provider: the wrapped key did not open");
            KeyError::Corrupt {
                reason: "the wrapped key did not open under the master",
            }
        })?;
        let Ok(key) = <[u8; KEY_BYTES]>::try_from(opened) else {
            lgwks_std::trace::debug!("provider: the opened key is not 32 bytes");
            return Err(KeyError::Corrupt {
                reason: "the opened key is not 32 bytes",
            });
        };
        Ok(DataKey::from_bytes(key))
    }
}

impl core::fmt::Debug for SoftwareProvider {
    /// The directory, the counts and the page-lock report: the shape of the
    /// provider without any secret it holds (CWE-532).
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (live, destroyed) = match self.state.lock() {
            Ok(state) => (state.keys.len(), state.destroyed.len()),
            Err(_) => (usize::MAX, usize::MAX),
        };
        formatter
            .debug_struct("SoftwareProvider")
            .field("dir", &self.dir)
            .field("live_keys", &live)
            .field("destroyed_keys", &destroyed)
            .field("page_lock", &self.page_lock_report())
            .finish()
    }
}

impl SoftwareProvider {
    /// Open the provider in `dir`, generating its master key on first use.
    ///
    /// Creates the directory owner-only when absent, reads `master.key` and
    /// `destroyed.log` when present, and truncates a torn tail of the log —
    /// a crash during an unsynced append, which was never acknowledged, so it
    /// is trimmed rather than replayed.
    ///
    /// # Errors
    ///
    /// [`KeyError::Storage`] when the directory or either file cannot be
    /// created, read or repaired; [`KeyError::Corrupt`] when the master file
    /// is present but is not a 32-byte key; [`KeyError::Transient`] when
    /// entropy for a first master is unavailable.
    pub fn open(dir: &Path) -> Result<Self, KeyError> {
        if let Err(error) = std::fs::create_dir_all(dir) {
            let refusal = Err(KeyError::Storage { cause: error });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider-open: the provider directory could not be created");
            return refusal;
        }
        if let Err(error) = restrict_dir(dir) {
            let refusal = Err(KeyError::Storage { cause: error });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider-open: the provider directory could not be restricted");
            return refusal;
        }
        let master_path = dir.join(MASTER_FILE);
        let master = match std::fs::read(&master_path) {
            Ok(bytes) => {
                let Ok(key) = <[u8; KEY_BYTES]>::try_from(bytes) else {
                    lgwks_std::trace::debug!("provider-open: the master file is not a 32-byte key");
                    return Err(KeyError::Corrupt {
                        reason: "the master file is not a 32-byte key",
                    });
                };
                DataKey::from_bytes(key)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let key = match lgwks_std::random::bytes::<KEY_BYTES>() {
                    Ok(key) => key,
                    Err(error) => {
                        lgwks_std::trace::debug!(error = ?error, "provider-open: no entropy for a first master key");
                        return Err(KeyError::Transient {
                            reason: "no entropy for a first master key",
                        });
                    }
                };
                let mut file = OpenOptions::new()
                    .create_new(true)
                    .write(true)
                    .open(&master_path)
                    .map_err(|error| {
                        lgwks_std::trace::debug!(error = ?error, "provider-open: the master file could not be created");
                        KeyError::Storage { cause: error }
                    })?;
                file.write_all(&key).map_err(|error| {
                    lgwks_std::trace::debug!(error = ?error, "provider-open: the master key could not be written");
                    KeyError::Storage { cause: error }
                })?;
                file.sync_all().map_err(|error| {
                    lgwks_std::trace::debug!(error = ?error, "provider-open: the master key could not be synced");
                    KeyError::Storage { cause: error }
                })?;
                drop(file);
                if let Err(error) = restrict_file(&master_path) {
                    let refusal = Err(KeyError::Storage { cause: error });
                    lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider-open: the master file could not be restricted");
                    return refusal;
                }
                DataKey::from_bytes(key)
            }
            Err(error) => {
                let refusal = Err(KeyError::Storage { cause: error });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider-open: the master file could not be read");
                return refusal;
            }
        };
        let log_path = dir.join(DESTROYED_FILE);
        if !log_path.exists() {
            let file = OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&log_path)
                .map_err(|error| {
                    lgwks_std::trace::debug!(error = ?error, "provider-open: the destruction log could not be created");
                    KeyError::Storage { cause: error }
                })?;
            drop(file);
            if let Err(error) = restrict_file(&log_path) {
                let refusal = Err(KeyError::Storage { cause: error });
                lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider-open: the destruction log could not be restricted");
                return refusal;
            }
        }
        let mut destroyed = HashSet::new();
        let raw = std::fs::read(&log_path).map_err(|error| {
            lgwks_std::trace::debug!(error = ?error, "provider-open: the destruction log could not be read");
            KeyError::Storage { cause: error }
        })?;
        let (identities, torn) = raw.as_chunks::<16>();
        for identity in identities {
            destroyed.insert(*identity);
        }
        if !torn.is_empty() {
            lgwks_std::trace::debug!(
                torn = torn.len(),
                "provider-open: trimming a torn tail of the destruction log"
            );
            let file = OpenOptions::new().write(true).open(&log_path).map_err(|error| {
                lgwks_std::trace::debug!(error = ?error, "provider-open: the torn log could not be reopened for repair");
                KeyError::Storage { cause: error }
            })?;
            let kept = u64::try_from(raw.len().saturating_sub(torn.len())).map_err(|cause| {
                lgwks_std::trace::debug!(error = ?cause, "provider-open: the kept log length does not fit u64");
                KeyError::Storage {
                    cause: std::io::Error::other(cause),
                }
            })?;
            file.set_len(kept)
                .map_err(|error| {
                    lgwks_std::trace::debug!(error = ?error, "provider-open: the torn log could not be trimmed");
                    KeyError::Storage { cause: error }
                })?;
            file.sync_all().map_err(|error| {
                lgwks_std::trace::debug!(error = ?error, "provider-open: the trimmed log could not be synced");
                KeyError::Storage { cause: error }
            })?;
        }
        let log = OpenOptions::new().append(true).open(&log_path).map_err(|error| {
            lgwks_std::trace::debug!(error = ?error, "provider-open: the destruction log could not be opened for append");
            KeyError::Storage { cause: error }
        })?;
        Ok(Self {
            dir: dir.to_path_buf(),
            state: std::sync::Mutex::new(ProviderState {
                master,
                keys: HashMap::new(),
                destroyed,
                log,
            }),
        })
    }

    /// What page locking promises here: the static report, printable by a
    /// caller choosing this provider. See the module docs for why it is not
    /// held.
    #[must_use]
    pub fn page_lock_report(&self) -> &'static str {
        PAGE_LOCK_REPORT
    }

    /// The directory holding the master key and the destruction log.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Lock the live state, poison-safe: a poisoned lock still guards
    /// consistent maps, because every critical section installs a complete
    /// value or returns before touching one — so the waiters report the
    /// poisoning as transient rather than strand the provider.
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, ProviderState>, KeyError> {
        match self.state.lock() {
            Ok(guard) => Ok(guard),
            Err(_) => {
                lgwks_std::trace::debug!(
                    "provider-lock: the state lock is poisoned; reporting transient"
                );
                Err(KeyError::Transient {
                    reason: "the provider state lock was poisoned",
                })
            }
        }
    }
}

impl KeyProvider for SoftwareProvider {
    /// Mint a fresh 32-byte data key for `tenant`.
    ///
    /// Draws until the identity is neither live nor destroyed, bounded at 256
    /// draws: past that the entropy source is repeating itself, and a key
    /// that reuses an identity would share across records, which this
    /// provider never does.
    fn provision(&self, tenant: &str) -> Result<(KeyId, DataKey), KeyError> {
        let mut state = self.lock()?;
        let mut draws = 0_u32;
        loop {
            draws = draws.saturating_add(1);
            if draws > 256 {
                lgwks_std::trace::debug!(
                    "provider-provision: the entropy source repeated past the draw bound"
                );
                return Err(KeyError::Transient {
                    reason: "the entropy source repeated past the draw bound",
                });
            }
            let id = match lgwks_std::random::bytes::<16>() {
                Ok(id) => id,
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "provider-provision: no entropy for a data key");
                    return Err(KeyError::Transient {
                        reason: "no entropy for a data key",
                    });
                }
            };
            if state.keys.contains_key(&id) || state.destroyed.contains(&id) {
                continue;
            }
            let key = match lgwks_std::random::bytes::<KEY_BYTES>() {
                Ok(key) => key,
                Err(error) => {
                    lgwks_std::trace::debug!(error = ?error, "provider-provision: no entropy for a data key");
                    return Err(KeyError::Transient {
                        reason: "no entropy for a data key",
                    });
                }
            };
            let tenant = tenant.to_owned();
            state
                .keys
                .insert(id, (tenant.clone(), DataKey::from_bytes(key)));
            let handed = DataKey::from_bytes(key);
            return Ok((KeyId { tenant, id }, handed));
        }
    }

    /// Seal the data key under the master, with a nonce derived from the key
    /// identity: the identity is unique per provision, so the nonce is fresh
    /// per wrap without spending entropy, and a second wrap of the same key
    /// seals the same bytes under the same nonce — identical output, no leak.
    fn wrap_key(&self, id: &KeyId) -> Result<Vec<u8>, KeyError> {
        let state = self.lock()?;
        let key = state.live_key(id)?;
        let mut nonce = [0_u8; NONCE_BYTES];
        nonce.copy_from_slice(&id.id[..NONCE_BYTES]);
        seal(&state.master, &nonce, key.bytes())
            .map_err(|error| {
                lgwks_std::trace::debug!(error = ?error, "provider-wrap: the seal step refused");
                KeyError::Transient {
                    reason: "the seal step refused",
                }
            })
            .map(|mut sealed| {
                let mut out = Vec::with_capacity(WRAPPED_BYTES);
                out.extend_from_slice(&nonce);
                out.append(&mut sealed);
                out
            })
    }

    /// Open `wrapped` for `id`: destroyed first, so a shredded key reads as
    /// destroyed however stale its wrapped bytes are.
    fn unwrap_key(&self, id: &KeyId, wrapped: &[u8]) -> Result<DataKey, KeyError> {
        let state = self.lock()?;
        state.live_key(id)?;
        state.open_wrapped(wrapped)
    }

    /// Destroy the key: record the destruction on the device first, then
    /// remove and zeroize the live copy. A device refusal leaves the key
    /// live, so the caller still reads the record as live — erased is only
    /// ever reported for a destruction the disk holds.
    fn destroy(&self, id: &KeyId) -> Result<(), KeyError> {
        let mut state = self.lock()?;
        if state.destroyed.contains(&id.id) {
            return Ok(());
        }
        if !state.keys.contains_key(&id.id) {
            let refusal = Err(KeyError::Missing {
                tenant: id.tenant.clone(),
                id: id.id,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "provider-destroy: no such key");
            return refusal;
        }
        state.log.write_all(&id.id).map_err(|error| {
            lgwks_std::trace::debug!(error = ?error, "provider-destroy: the destruction could not be written");
            KeyError::Storage { cause: error }
        })?;
        state.log.sync_all().map_err(|error| {
            lgwks_std::trace::debug!(error = ?error, "provider-destroy: the destruction could not be synced");
            KeyError::Storage { cause: error }
        })?;
        let _removed = state.keys.remove(&id.id);
        state.destroyed.insert(id.id);
        Ok(())
    }

    /// Restore the live key `wrapped` names: open it under the master and
    /// keep it, unless the identity was destroyed — then the note is a
    /// no-op, because a destruction is never undone by a replay.
    fn note_wrapped(&self, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError> {
        let mut state = self.lock()?;
        if state.destroyed.contains(&id.id) {
            return Ok(());
        }
        if state.keys.contains_key(&id.id) {
            return Ok(());
        }
        let key = state.open_wrapped(wrapped)?;
        state.keys.insert(id.id, (id.tenant.clone(), key));
        Ok(())
    }
}
