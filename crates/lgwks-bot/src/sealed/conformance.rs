//! The provider conformance kit: the rules every [`KeyProvider`] keeps,
//! run against one provider and reported rule by rule.
//!
//! A provider crate calls [`check`] against itself. The kit stands in for the
//! store: it mints, seals, wraps, unwraps, destroys, restarts the provider and
//! notes every wrapped key back, all in an order one seed fixes. Each step's
//! outcome class (never a key, never an identity) feeds [`Conformance::trace_hash`],
//! so two correct providers give the same hash for the same seed.
//!
//! [`KeyProvider`]: crate::sealed::keys::KeyProvider
//! [`check`]: crate::sealed::conformance::check
//! [`Conformance::trace_hash`]: crate::sealed::conformance::Conformance::trace_hash

use std::sync::Arc;

use lgwks_std::seeded::Seeded;

use super::envelope::{DataKey, NONCE_BYTES, open, seal};
use super::keys::{KeyError, KeyId, KeyProvider};

/// The most records one seed provisions.
const KIT_RECORDS: u64 = 32;

/// How many records past the first two a seed may add: a seed provisions
/// 2 to 31 records, inside [`KIT_RECORDS`].
const KIT_EXTRA: u64 = 30;

/// The most identities one [`mint_ids`] call provisions.
const MINT_IDS_MAX: usize = 4096;

/// The two tenants the kit provisions for, alternately.
const TENANTS: [&str; 2] = ["kit-a", "kit-b"];

/// A rule the provider broke, without key bytes.
///
/// ```no_run
/// # use std::sync::Arc;
/// # use lgwks_bot::sealed::conformance::{self, Violation};
/// # use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
/// # let dir = std::env::temp_dir().join("lgwks-kit-doc");
/// # let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> { Ok(Arc::new(SoftwareProvider::open(&dir)?)) };
/// let found = conformance::check(&mut open, 7);
/// assert!(!found.violations().contains(&Violation::ForeignTenantOpens));
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Violation {
    /// The provider could not be opened, first or after the restart.
    OpenRefused,
    /// `provision` refused a fresh key.
    ProvisionRefused,
    /// Two provisions returned one identity.
    DuplicateIdentity,
    /// An identity came back for another tenant than the one asked.
    WrongTenant,
    /// A wrap, unwrap or open did not return what the key sealed.
    RoundTripMismatch,
    /// Another tenant's identity opened a key.
    ForeignTenantOpens,
    /// `destroy` refused a live key, or a second `destroy` was not a no-op.
    DestroyRefused,
    /// A destroyed key still opened, or refused as something other than
    /// [`KeyError::Destroyed`].
    DestroyedStillOpens,
    /// `note_wrapped` refused a key this provider wrapped.
    NoteRefused,
    /// A live key did not open after the restart.
    RestartLostKey,
}

impl core::fmt::Display for Violation {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match *self {
            Self::OpenRefused => "the provider could not be opened",
            Self::ProvisionRefused => "provision refused a fresh key",
            Self::DuplicateIdentity => "two provisions returned one identity",
            Self::WrongTenant => "an identity came back for another tenant",
            Self::RoundTripMismatch => "a wrapped key did not open its own seal",
            Self::ForeignTenantOpens => "another tenant's identity opened a key",
            Self::DestroyRefused => "destroy refused a key, or a second destroy was not a no-op",
            Self::DestroyedStillOpens => "a destroyed key still opened",
            Self::NoteRefused => "note_wrapped refused a key this provider wrapped",
            Self::RestartLostKey => "a live key did not open after a restart",
        })
    }
}

/// What [`check`] found for one seed.
///
/// ```no_run
/// # use std::sync::Arc;
/// # use lgwks_bot::sealed::conformance::{self, Conformance};
/// # use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
/// # let dir = std::env::temp_dir().join("lgwks-kit-doc");
/// # let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> { Ok(Arc::new(SoftwareProvider::open(&dir)?)) };
/// let found: Conformance = conformance::check(&mut open, 7);
/// assert!(found.passed(), "{:?}", found.violations());
/// ```
#[derive(Debug)]
pub struct Conformance {
    /// Every rule the provider broke, in the order the kit met them.
    violations: Vec<Violation>,
    /// One byte per step: the outcome class the step reached.
    trace: Vec<u8>,
}

impl Conformance {
    /// Whether the provider kept every rule.
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use lgwks_bot::sealed::conformance;
    /// # use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
    /// # let dir = std::env::temp_dir().join("lgwks-kit-doc");
    /// # let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> { Ok(Arc::new(SoftwareProvider::open(&dir)?)) };
    /// assert!(conformance::check(&mut open, 7).passed());
    /// ```
    #[must_use]
    pub fn passed(&self) -> bool {
        self.violations.is_empty()
    }

    /// Every rule the provider broke.
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use lgwks_bot::sealed::conformance;
    /// # use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
    /// # let dir = std::env::temp_dir().join("lgwks-kit-doc");
    /// # let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> { Ok(Arc::new(SoftwareProvider::open(&dir)?)) };
    /// assert!(conformance::check(&mut open, 7).violations().is_empty());
    /// ```
    #[must_use]
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// The blake3 digest of the outcome classes, equal across correct
    /// providers for one seed.
    ///
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use lgwks_bot::sealed::conformance;
    /// # use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
    /// # let (one, two) = (std::env::temp_dir().join("lgwks-kit-a"), std::env::temp_dir().join("lgwks-kit-b"));
    /// # let mut first = || -> Result<Arc<dyn KeyProvider>, KeyError> { Ok(Arc::new(SoftwareProvider::open(&one)?)) };
    /// # let mut second = || -> Result<Arc<dyn KeyProvider>, KeyError> { Ok(Arc::new(SoftwareProvider::open(&two)?)) };
    /// let a = conformance::check(&mut first, 7).trace_hash();
    /// let b = conformance::check(&mut second, 7).trace_hash();
    /// assert_eq!(a, b, "two correct providers run one trace for one seed");
    /// ```
    #[must_use]
    pub fn trace_hash(&self) -> [u8; 32] {
        *lgwks_std::hash::blake3(&self.trace).as_bytes()
    }

    /// Record `class`, and `violation` when the step broke a rule.
    fn step(&mut self, class: u8, violation: Option<Violation>) {
        self.trace.push(class);
        if let Some(violation) = violation {
            lgwks_std::trace::debug!(?violation, "conformance: the provider broke a rule");
            self.violations.push(violation);
        }
    }
}

/// One provisioned record: its identity, its wrapped key, the plaintext it
/// sealed and that seal, and whether the kit destroyed it.
struct Record {
    /// The identity the provider minted.
    id: KeyId,
    /// The provider's wrapped bytes for it.
    wrapped: Vec<u8>,
    /// The seed-derived plaintext.
    plain: Vec<u8>,
    /// The plaintext sealed under the record's key.
    sealed: Vec<u8>,
    /// Whether the kit destroyed the key.
    destroyed: bool,
}

/// The nonce a record seals under: its identity's leading bytes.
fn nonce_of(id: &KeyId) -> Option<[u8; NONCE_BYTES]> {
    id.id().first_chunk::<NONCE_BYTES>().copied()
}

/// Whether `answer` is a key that opens `record`'s seal to its plaintext.
fn opens(answer: Result<DataKey, KeyError>, record: &Record) -> bool {
    let key = match answer {
        Ok(key) => key,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "conformance: the key did not unwrap");
            return false;
        }
    };
    open_record(&key, record).is_some_and(|opened| opened == record.plain)
}

/// `record`'s seal opened under `key`, or `None` when it does not open.
fn open_record(key: &DataKey, record: &Record) -> Option<Vec<u8>> {
    let nonce = nonce_of(&record.id)?;
    match open(key, &nonce, &record.sealed) {
        Ok(opened) => Some(opened),
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "conformance: the seal did not open");
            None
        }
    }
}

/// Seal `plain` under `key` with `id`'s nonce.
fn seal_for(id: &KeyId, key: &DataKey, plain: &[u8]) -> Option<Vec<u8>> {
    let nonce = nonce_of(id)?;
    match seal(key, &nonce, plain) {
        Ok(sealed) => Some(sealed),
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "conformance: the seal step refused");
            None
        }
    }
}

/// Run every rule against the provider `open_provider` builds, for `seed`.
///
/// `open_provider` is called once to start and once more to model a
/// restart, so it must reopen the same durable state.
///
/// ```no_run
/// use std::sync::Arc;
/// use lgwks_bot::sealed::conformance;
/// use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
///
/// let dir = std::env::temp_dir().join("lgwks-kit-doc");
/// let mut open = || -> Result<Arc<dyn KeyProvider>, KeyError> {
///     Ok(Arc::new(SoftwareProvider::open(&dir)?))
/// };
/// let found = conformance::check(&mut open, 7);
/// assert!(found.passed(), "{:?}", found.violations());
/// ```
pub fn check(
    open_provider: &mut dyn FnMut() -> Result<Arc<dyn KeyProvider>, KeyError>,
    seed: u64,
) -> Conformance {
    let mut kit = Conformance {
        violations: Vec::new(),
        trace: Vec::new(),
    };
    let mut rng = Seeded::from_seed(seed);
    let Ok(provider) = open_provider() else {
        kit.step(0, Some(Violation::OpenRefused));
        return kit;
    };
    let count = match rng.next_u64().checked_rem(KIT_EXTRA) {
        Some(extra) => extra.saturating_add(2),
        None => KIT_RECORDS,
    };
    let mut records: Vec<Record> = Vec::new();
    for index in 0..count {
        // The two tenants alternate; which one goes first is symmetric.
        let side = index.is_multiple_of(2);
        let [first, second] = TENANTS;
        let (tenant, other) = if side {
            (second, first)
        } else {
            (first, second)
        };
        let Ok((id, key)) = provider.provision(tenant) else {
            kit.step(1, Some(Violation::ProvisionRefused));
            continue;
        };
        let duplicate = records.iter().any(|record| record.id.id() == id.id());
        kit.step(2, duplicate.then_some(Violation::DuplicateIdentity));
        kit.step(3, (id.tenant() != tenant).then_some(Violation::WrongTenant));
        let plain = rng.next_u64().to_le_bytes().to_vec();
        let sealed = seal_for(&id, &key, &plain);
        drop(key);
        let (Some(sealed), Ok(wrapped)) = (sealed, provider.wrap_key(&id)) else {
            kit.step(4, Some(Violation::RoundTripMismatch));
            continue;
        };
        let record = Record {
            id,
            wrapped,
            plain,
            sealed,
            destroyed: false,
        };
        let opened = opens(provider.unwrap_key(&record.id, &record.wrapped), &record);
        kit.step(5, (!opened).then_some(Violation::RoundTripMismatch));
        let foreign = KeyId::new(other.to_owned(), record.id.id());
        let leaked = provider.unwrap_key(&foreign, &record.wrapped).is_ok();
        kit.step(6, leaked.then_some(Violation::ForeignTenantOpens));
        records.push(record);
    }
    // The first record is always spared, so a restart has a live key to
    // restore; every other one is destroyed on the seed's coin.
    for record in records.iter_mut().skip(1) {
        if rng.next_u64().is_multiple_of(2) {
            continue;
        }
        let first = provider.destroy(&record.id).is_ok();
        let again = provider.destroy(&record.id).is_ok();
        kit.step(7, (!(first && again)).then_some(Violation::DestroyRefused));
        record.destroyed = first;
        let refused = matches!(
            provider.unwrap_key(&record.id, &record.wrapped),
            Err(KeyError::Destroyed { .. })
        );
        kit.step(8, (!refused).then_some(Violation::DestroyedStillOpens));
    }
    drop(provider);
    let Ok(provider) = open_provider() else {
        kit.step(9, Some(Violation::OpenRefused));
        return kit;
    };
    for record in &records {
        let noted = provider.note_wrapped(&record.id, &record.wrapped).is_ok();
        kit.step(10, (!noted).then_some(Violation::NoteRefused));
        let answer = provider.unwrap_key(&record.id, &record.wrapped);
        if record.destroyed {
            let refused = matches!(answer, Err(KeyError::Destroyed { .. }));
            kit.step(11, (!refused).then_some(Violation::DestroyedStillOpens));
        } else {
            let live = opens(answer, record);
            kit.step(12, (!live).then_some(Violation::RestartLostKey));
        }
    }
    kit
}

/// Unwrap `wrapped` for `id` with a fresh mint and drop the key at once:
/// whether the provider would answer the store, without handing anyone the key.
///
/// # Errors
///
/// Whatever the provider's `unwrap_with` reports.
///
/// ```no_run
/// # use lgwks_bot::sealed::conformance::{mint_ids, try_unwrap};
/// # use lgwks_bot::sealed::keys::{KeyError, KeyProvider, SoftwareProvider};
/// # fn main() -> Result<(), KeyError> {
/// let provider = SoftwareProvider::open(&std::env::temp_dir().join("lgwks-kit-doc"))?;
/// for id in mint_ids(&provider, "acme", 2)? {
///     try_unwrap(&provider, &id, &provider.wrap_key(&id)?)?;
/// }
/// # Ok(())
/// # }
/// ```
pub fn try_unwrap(provider: &dyn KeyProvider, id: &KeyId, wrapped: &[u8]) -> Result<(), KeyError> {
    let key = provider.unwrap_key(id, wrapped)?;
    drop(key);
    Ok(())
}

/// Provision `count` keys for `tenant`, each with its own mint, and return
/// their identities; every key is dropped, and so zeroized, at once.
///
/// # Errors
///
/// [`KeyError::Transient`] when `count` is past 4,096, and
/// whatever the provider's `provision` reports.
///
/// ```no_run
/// # use lgwks_bot::sealed::conformance::mint_ids;
/// # use lgwks_bot::sealed::keys::{KeyError, SoftwareProvider};
/// # fn main() -> Result<(), KeyError> {
/// let provider = SoftwareProvider::open(&std::env::temp_dir().join("lgwks-kit-doc"))?;
/// let ids = mint_ids(&provider, "acme", 2)?;
/// assert_ne!(ids[0].id(), ids[1].id(), "two provisions, two identities");
/// # Ok(())
/// # }
/// ```
pub fn mint_ids(
    provider: &dyn KeyProvider,
    tenant: &str,
    count: usize,
) -> Result<Vec<KeyId>, KeyError> {
    if count > MINT_IDS_MAX {
        lgwks_std::trace::debug!(count, "conformance: mint_ids was asked past its bound");
        return Err(KeyError::Transient {
            reason: "mint_ids was asked for more than MINT_IDS_MAX keys",
        });
    }
    let mut ids = Vec::with_capacity(count);
    for _ in 0..count {
        let (id, key) = provider.provision(tenant)?;
        drop(key);
        ids.push(id);
    }
    Ok(ids)
}
