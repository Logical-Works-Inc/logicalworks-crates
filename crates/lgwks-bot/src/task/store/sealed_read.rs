//! The run-store half of sealed records: what a sealed step read shows and
//! what a shred did.
//!
//! The methods live on [`RunStore`](crate::task::RunStore) beside the append
//! path they seal — [`RunStore::open_sealed`](crate::task::RunStore::open_sealed),
//! [`RunStore::append_with_expiry`](crate::task::RunStore::append_with_expiry),
//! [`RunStore::read_sealed`](crate::task::RunStore::read_sealed),
//! [`RunStore::shred_record`](crate::task::RunStore::shred_record),
//! [`RunStore::erase_tenant_records`](crate::task::RunStore::erase_tenant_records),
//! [`RunStore::sweep_once`](crate::task::RunStore::sweep_once) and
//! [`RunStore::verify_sealed_chain`](crate::task::RunStore::verify_sealed_chain) —
//! because they reach the handle's own index, fence and chain. This module
//! holds the two types those methods answer in, beside the store that answers
//! them, so the sealed kernel names no run-store or script type; a caller
//! reaches both through `lgwks_bot::task`.

use crate::effect::RunId;
use crate::script::{StepKey, run_store::StoredValue};

/// What one sealed step reads as: the live value, or the tombstone of a
/// shredded record.
///
/// There is no absent arm: an unknown step is [`StoreError::SealedUnknown`](crate::task::StoreError::SealedUnknown),
/// never a miss that would let a finished step run again.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use lgwks_bot::effect::RunId;
/// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
/// use lgwks_bot::script::{Scope, Tenant};
/// use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend, SealedStepRead};
///
/// let dir = std::env::temp_dir().join("lgwks-doc-sealed-step-read");
/// std::fs::create_dir_all(&dir)?;
/// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
/// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
/// let definition = DefinitionIdentity::new("doc", 1, lgwks_std::hash::blake3(b"doc"), 1);
/// let run = RunId::from_hex("00000000000000000000000000000002")?;
/// let key = Scope::root(Tenant::new("acme")?).enter("greeting")?.key();
/// store.append_with_expiry(SealedAppend::new(
///     "acme", run, key, "greeting", &definition, b"hello".to_vec(), None,
/// ))?;
///
/// // Live while its key stands, the tombstone once the record is shredded.
/// assert!(matches!(store.read_sealed("acme", run, key)?, SealedStepRead::Live { .. }));
/// store.shred_record("acme", run, key)?;
/// assert!(matches!(store.read_sealed("acme", run, key)?, SealedStepRead::Erased { .. }));
/// drop(store);
/// std::fs::remove_dir_all(&dir)?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SealedStepRead {
    /// The step is live: its path and its archived value, opened under its
    /// own data key.
    Live {
        /// The step's path and archived value.
        value: StoredValue,
    },
    /// The step was shredded: its key destroyed, its value unreadable, its
    /// place in the chain still verified.
    Erased {
        /// The run the shredded step belongs to.
        run: RunId,
        /// The shredded step's key.
        key: StepKey,
    },
}

/// What a shred did: shredded now, or already a tombstone.
///
/// Either way the step reads as erased afterwards, which is what makes the
/// second shred a no-op rather than a second destruction.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::Arc;
/// use lgwks_bot::effect::RunId;
/// use lgwks_bot::sealed::keys::{KeyProvider, SoftwareProvider};
/// use lgwks_bot::script::{Scope, Tenant};
/// use lgwks_bot::task::{DefinitionIdentity, RunStore, SealedAppend, ShredOutcome};
///
/// let dir = std::env::temp_dir().join("lgwks-doc-shred-outcome");
/// std::fs::create_dir_all(&dir)?;
/// let provider: Arc<dyn KeyProvider> = Arc::new(SoftwareProvider::open(&dir)?);
/// let store = RunStore::open_sealed(dir.join("store.runstore"), provider)?;
/// let definition = DefinitionIdentity::new("doc", 1, lgwks_std::hash::blake3(b"doc"), 1);
/// let run = RunId::from_hex("00000000000000000000000000000002")?;
/// let key = Scope::root(Tenant::new("acme")?).enter("secret")?.key();
/// store.append_with_expiry(SealedAppend::new(
///     "acme", run, key, "secret", &definition, b"hush".to_vec(), None,
/// ))?;
///
/// // The first shred destroys the key; the second finds the tombstone.
/// assert_eq!(store.shred_record("acme", run, key)?, ShredOutcome::Shredded { run, key });
/// assert_eq!(store.shred_record("acme", run, key)?, ShredOutcome::AlreadyErased { run, key });
/// drop(store);
/// std::fs::remove_dir_all(&dir)?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShredOutcome {
    /// The key was destroyed and the tombstone committed by this call.
    Shredded {
        /// The run the shredded step belongs to.
        run: RunId,
        /// The shredded step's key.
        key: StepKey,
    },
    /// The step was already a tombstone: nothing was destroyed and nothing
    /// was written.
    AlreadyErased {
        /// The run the shredded step belongs to.
        run: RunId,
        /// The shredded step's key.
        key: StepKey,
    },
}
