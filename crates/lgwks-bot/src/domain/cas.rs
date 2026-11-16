//! `cas` owns the versioned-record write domain: the [`Execute`](crate::verb::Execute)
//! adapter over [`RecordStore`](crate::cas::RecordStore).
//!
//! The store is a kernel part and knows nothing of verbs; this is the one place
//! that turns its outcomes into the verb's typed errors, so a lower layer never
//! names an upper one (#990).

use std::sync::Arc;

use crate::cap::{Auth, Cap};
use crate::cas::{CasError, CasInput, CasOutput, RecordStore};
use crate::ecs::EcsError;
use crate::error::{DispatchCertainty, KernelError};
use crate::verb;

/// The domain this adapter reports its refusals under.
const DOMAIN: &str = "cas::store";

/// An [`Execute`](crate::verb::Execute) adapter that writes one versioned
/// shared record.
///
/// The writer of shared records the concurrent-editors row asks for: the
/// expected-version precondition rides in the input, and a lost write is the
/// typed [`KernelError::Conflict`] outcome —
/// recorded in the store's conflict log, never retried blindly.
///
/// # Examples
///
/// ```
/// use std::sync::Arc;
/// use lgwks_bot::cas::{CasInput, RecordStore};
/// use lgwks_bot::domain::cas::CasWrite;
/// use lgwks_bot::cap::Cap;
/// use lgwks_bot::gate::GrantSet;
/// use lgwks_bot::verb::Execute;
///
/// let path = std::env::temp_dir().join(format!("lgwks-cas-write-{}", std::process::id()));
/// let adapter = CasWrite::new(Arc::new(RecordStore::open(&path)?));
/// let auth = GrantSet::empty().grant(Cap::fs()).issue(&[Cap::fs()])?;
///
/// let first = CasInput::new("draft", 0, "one");
/// let output = lgwks_bot::rt::runtime::block_on(adapter.execute_action((auth.clone(), &first)))?;
/// assert_eq!(output.version(), 1);
///
/// let stale = CasInput::new("draft", 0, "two");
/// assert!(lgwks_bot::rt::runtime::block_on(adapter.execute_action((auth, &stale))).is_err());
/// if path.exists() {
///     std::fs::remove_file(&path)?;
/// }
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Debug)]
pub struct CasWrite {
    /// The store the writes land in, shared so one adapter serves every
    /// record on one file.
    store: Arc<RecordStore>,
    /// Forced to `[bot.fs]`: the store is a file, and no constructor omits
    /// the capability.
    caps: Vec<Cap>,
}

impl CasWrite {
    /// Serve versioned writes against `store`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use lgwks_bot::cas::RecordStore;
    /// use lgwks_bot::domain::cas::CasWrite;
    ///
    /// let path = std::env::temp_dir().join(format!("lgwks-cas-new-{}", std::process::id()));
    /// let adapter = CasWrite::new(Arc::new(RecordStore::open(&path)?));
    /// assert_eq!(Arc::strong_count(adapter.store()), 1);
    /// if path.exists() {
    ///     std::fs::remove_file(&path)?;
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn new(store: Arc<RecordStore>) -> Self {
        Self {
            store,
            caps: vec![Cap::fs()],
        }
    }

    /// The store the writes land in.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::Arc;
    /// use lgwks_bot::cas::RecordStore;
    /// use lgwks_bot::domain::cas::CasWrite;
    ///
    /// let path = std::env::temp_dir().join(format!("lgwks-cas-store-{}", std::process::id()));
    /// let shared = Arc::new(RecordStore::open(&path)?);
    /// let adapter = CasWrite::new(Arc::clone(&shared));
    /// assert!(Arc::ptr_eq(adapter.store(), &shared));
    /// if path.exists() {
    ///     std::fs::remove_file(&path)?;
    /// }
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    #[must_use]
    pub fn store(&self) -> &Arc<RecordStore> {
        &self.store
    }
}

/// What a refused store write is, as the verb reports it.
///
/// A lost compare-and-set is the typed conflict naming both versions; a store
/// that could not read or persist is a refusal before anything was applied.
fn refusal(error: CasError) -> EcsError {
    let domain = DOMAIN.to_owned();
    match error {
        CasError::Conflict {
            record,
            expected,
            found,
        } => EcsError::from(KernelError::Conflict {
            domain,
            record,
            expected,
            found,
        }),
        CasError::Corrupt { cause } | CasError::Io { cause } => {
            EcsError::from(KernelError::DomainError {
                domain,
                certainty: DispatchCertainty::Refused,
                cause,
            })
        }
    }
}

impl verb::Execute for CasWrite {
    type Input = CasInput;
    type Output = CasOutput;

    fn required_caps(&self) -> &[Cap] {
        &self.caps
    }

    fn effect_lifetime(&self) -> verb::EffectLifetime {
        verb::EffectLifetime::External
    }

    async fn execute_action(&self, call: (Auth, &Self::Input)) -> Result<Self::Output, EcsError> {
        let (auth, input) = call;
        auth.check(&self.caps)?;
        // Synchronous under the in-process mutex, awaited for nothing: the
        // critical section holds no `.await`, so there is no lock held across
        // one by construction.
        match self
            .store
            .write(input.record(), input.expected(), input.value())
        {
            Ok(version) => Ok(CasOutput::installed(input.record(), version)),
            Err(error) => {
                let refused = refusal(error);
                lgwks_std::trace::debug!(
                    error = ?refused,
                    "execute_action: returning an error to the caller"
                );
                Err(refused)
            }
        }
    }

    fn domain_id(&self) -> &str {
        DOMAIN
    }
}
