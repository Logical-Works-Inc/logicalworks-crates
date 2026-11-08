//! A scope's stop, recorded in the consumer's own journal and restored from it.
//!
//! A cancellation token lives in memory, so a consumer whose cancellation is a
//! journaled transition keeps two copies of one fact. Here the journal holds the fact:
//! [`JournaledCancel::request`] appends [`CancelStage::Requested`] and then fires the
//! token, and [`JournaledCancel::restore`] folds the journal and fires a fresh token
//! when the stop was already asked for. A record names its stage in one fixed order,
//! so a restart cannot read a finish that was never requested.
//!
//! Only `request` records. [`Scope::cancel`] and a supervisor's token still stop a run
//! in memory, and a restart does not remember them. A continued journal is read from
//! its checkpoint, so the checkpoint must carry the stage of any stop still open.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use lgwks_std::wire::rkyv::{Archive, Deserialize, Serialize};

use super::scope::{Scope, Tenant};
use crate::journal::{Durable, JournalError, RecordJournal};
use crate::rt::sync::CancellationToken;

/// Where a stop stands, in the only order it may move.
///
/// # Examples
///
/// ```
/// use lgwks_bot::script::CancelStage;
///
/// assert!(CancelStage::Requested < CancelStage::Finished);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Archive, Serialize, Deserialize)]
#[rkyv(crate = lgwks_std::wire::rkyv, compare(PartialEq), derive(Debug))]
#[non_exhaustive]
pub enum CancelStage {
    /// Someone asked for the stop, and the token has fired.
    Requested,
    /// The work is winding down.
    Cancelling,
    /// The work has stopped.
    Finished,
}

/// How a consumer's own record says a stop moved.
///
/// # Examples
///
/// ```
/// use lgwks_bot::journal::Durable;
/// use lgwks_bot::script::{CancelRecord, CancelStage};
///
/// fn is_mark<R: CancelRecord + Durable>(record: &R) -> bool {
///     record.as_cancel().is_some()
/// }
/// ```
pub trait CancelRecord: Sized {
    /// The record that says `name`'s stop reached `stage`.
    fn cancel(name: &str, stage: CancelStage) -> Self;

    /// What this record says about a stop, or `None` for any other record.
    fn as_cancel(&self) -> Option<(&str, CancelStage)>;
}

/// Why a stop did not move.
///
/// # Examples
///
/// ```
/// use lgwks_bot::script::{CancelError, CancelStage};
///
/// let refused = CancelError::OutOfOrder { held: None, asked: CancelStage::Finished };
/// assert!(refused.to_string().contains("Finished"));
/// ```
#[derive(Debug)]
#[non_exhaustive]
pub enum CancelError {
    /// The journal refused. After a refused request the token has still fired, so
    /// this process stops, and only a later request that lands survives a restart.
    Journal(JournalError),
    /// This stage does not follow the one held. Nothing was written.
    OutOfOrder {
        /// The stage held.
        held: Option<CancelStage>,
        /// The stage asked for.
        asked: CancelStage,
    },
    /// A record in the journal does not follow the ones before it.
    Diverged {
        /// The record's sequence.
        sequence: u64,
        /// The stage held when it was read.
        held: Option<CancelStage>,
        /// The stage it names.
        found: CancelStage,
    },
}

impl fmt::Display for CancelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Journal(ref cause) => write!(f, "the journal refused: {cause}"),
            Self::OutOfOrder { held, asked } => {
                write!(f, "a stop at {held:?} cannot move to {asked:?}")
            }
            Self::Diverged {
                sequence,
                held,
                found,
            } => write!(
                f,
                "record {sequence} names {found:?} for a stop at {held:?}"
            ),
        }
    }
}

impl std::error::Error for CancelError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Journal(ref cause) => Some(cause),
            Self::OutOfOrder { .. } | Self::Diverged { .. } => None,
        }
    }
}

/// The stage that follows `held`.
const fn after(held: Option<CancelStage>) -> CancelStage {
    match held {
        None => CancelStage::Requested,
        Some(CancelStage::Requested) => CancelStage::Cancelling,
        Some(CancelStage::Cancelling | CancelStage::Finished) => CancelStage::Finished,
    }
}

/// A lock that a panicking holder cannot take away: the value is a stage or a journal
/// that answers for itself.
fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// One named stop, whose stages live in a journal the consumer also writes to.
///
/// Cloning shares the stop.
///
/// # Examples
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// use std::sync::{Arc, Mutex};
///
/// use lgwks_bot::journal::RecordJournal;
/// use lgwks_bot::rt::sync::CancellationToken;
/// use lgwks_bot::script::{CancelRecord, CancelStage, JournaledCancel, Tenant};
/// use lgwks_std::wire::rkyv::{Archive, Deserialize, Serialize};
///
/// #[derive(Archive, Serialize, Deserialize)]
/// #[rkyv(crate = lgwks_std::wire::rkyv)]
/// struct Mark(String, CancelStage);
///
/// impl CancelRecord for Mark {
///     fn cancel(name: &str, stage: CancelStage) -> Self {
///         Self(name.to_owned(), stage)
///     }
///     fn as_cancel(&self) -> Option<(&str, CancelStage)> {
///         Some((&self.0, self.1))
///     }
/// }
///
/// let path = std::env::temp_dir().join(format!("cancel-doc-{}", std::process::id()));
/// # drop(std::fs::remove_file(&path));
/// let journal = Arc::new(Mutex::new(RecordJournal::<Mark>::open(&path)?));
/// let stop = JournaledCancel::restore(Arc::clone(&journal), "run-1", CancellationToken::new())?;
/// stop.request()?;
/// stop.begin()?;
/// assert_eq!(stop.finish()?, CancelStage::Finished);
///
/// // A restart finds the stop, and a step cannot start under it.
/// let again = JournaledCancel::restore(journal, "run-1", CancellationToken::new())?;
/// assert_eq!(again.stage(), Some(CancelStage::Finished));
/// assert!(again.token().is_cancelled());
/// assert!(again.scope(Tenant::new("acme")?).enter("step").is_err());
/// # drop(again);
/// # drop(std::fs::remove_file(&path));
/// # Ok(())
/// # }
/// ```
pub struct JournaledCancel<R> {
    /// The journal, shared with whatever else the consumer records.
    journal: Arc<Mutex<RecordJournal<R>>>,
    /// The name records use for this stop.
    name: Arc<str>,
    /// What the records say so far.
    stage: Arc<Mutex<Option<CancelStage>>>,
    /// What stops the work.
    token: CancellationToken,
}

impl<R> Clone for JournaledCancel<R> {
    fn clone(&self) -> Self {
        Self {
            journal: Arc::clone(&self.journal),
            name: Arc::clone(&self.name),
            stage: Arc::clone(&self.stage),
            token: self.token.clone(),
        }
    }
}

impl<R: CancelRecord + Durable> JournaledCancel<R> {
    /// Fold `journal`'s live generation for `name`, firing `token` when it was asked
    /// for already.
    ///
    /// # Errors
    ///
    /// [`CancelError::Journal`] when a record cannot be read, and
    /// [`CancelError::Diverged`] when a stage does not follow the one before it. A
    /// record repeating the stage held is allowed.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::sync::{Arc, Mutex};
    ///
    /// use lgwks_bot::journal::{Durable, RecordJournal};
    /// use lgwks_bot::rt::sync::CancellationToken;
    /// use lgwks_bot::script::{CancelError, CancelRecord, JournaledCancel};
    ///
    /// fn resume<R: CancelRecord + Durable>(
    ///     journal: Arc<Mutex<RecordJournal<R>>>,
    /// ) -> Result<JournaledCancel<R>, CancelError> {
    ///     JournaledCancel::restore(journal, "run-1", CancellationToken::new())
    /// }
    /// ```
    pub fn restore(
        journal: Arc<Mutex<RecordJournal<R>>>,
        name: &str,
        token: CancellationToken,
    ) -> Result<Self, CancelError> {
        let mut stage = None;
        for entry in locked(&journal).replay().map_err(CancelError::Journal)? {
            let entry = entry.map_err(CancelError::Journal)?;
            let Some((named, found)) = entry.record().as_cancel() else {
                continue;
            };
            if named != name || stage == Some(found) {
                continue;
            }
            if after(stage) != found {
                let sequence = entry.position().sequence();
                let refusal = Err(CancelError::Diverged {
                    sequence,
                    held: stage,
                    found,
                });
                lgwks_std::trace::debug!(
                    sequence,
                    "JournaledCancel::restore: a stage out of order"
                );
                return refusal;
            }
            stage = Some(found);
        }
        if stage.is_some() {
            token.cancel();
        }
        Ok(Self {
            journal,
            name: Arc::from(name),
            stage: Arc::new(Mutex::new(stage)),
            token,
        })
    }

    /// Record that the stop was asked for, then fire the token.
    ///
    /// Asking again answers the stage held and writes nothing.
    ///
    /// # Errors
    ///
    /// [`CancelError::Journal`], and the token has fired anyway: stopping is never
    /// held back by a full disk. Ask again to record it.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::Durable;
    /// use lgwks_bot::script::{CancelError, CancelRecord, CancelStage, JournaledCancel};
    ///
    /// fn stop<R: CancelRecord + Durable>(run: &JournaledCancel<R>) -> Result<CancelStage, CancelError> {
    ///     run.request()
    /// }
    /// ```
    pub fn request(&self) -> Result<CancelStage, CancelError> {
        let recorded = self.advance(CancelStage::Requested);
        self.token.cancel();
        recorded
    }

    /// Record that the work is winding down.
    ///
    /// # Errors
    ///
    /// [`CancelError::OutOfOrder`] before a request is recorded, and
    /// [`CancelError::Journal`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::Durable;
    /// use lgwks_bot::script::{CancelError, CancelRecord, CancelStage, JournaledCancel};
    ///
    /// fn wind_down<R: CancelRecord + Durable>(run: &JournaledCancel<R>) -> Result<CancelStage, CancelError> {
    ///     run.begin()
    /// }
    /// ```
    pub fn begin(&self) -> Result<CancelStage, CancelError> {
        self.advance(CancelStage::Cancelling)
    }

    /// Record that the work has stopped.
    ///
    /// # Errors
    ///
    /// [`CancelError::OutOfOrder`] before winding down is recorded, and
    /// [`CancelError::Journal`].
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::journal::Durable;
    /// use lgwks_bot::script::{CancelError, CancelRecord, CancelStage, JournaledCancel};
    ///
    /// fn done<R: CancelRecord + Durable>(run: &JournaledCancel<R>) -> Result<CancelStage, CancelError> {
    ///     run.finish()
    /// }
    /// ```
    pub fn finish(&self) -> Result<CancelStage, CancelError> {
        self.advance(CancelStage::Finished)
    }

    /// Move to `asked`, writing a record only when that is a step forward.
    fn advance(&self, asked: CancelStage) -> Result<CancelStage, CancelError> {
        let mut journal = locked(&self.journal);
        let mut stage = locked(&self.stage);
        if let Some(held) = *stage
            && held >= asked
        {
            return Ok(held);
        }
        if after(*stage) != asked {
            let refusal = Err(CancelError::OutOfOrder {
                held: *stage,
                asked,
            });
            lgwks_std::trace::debug!(error = ?refusal.as_ref().err(), "JournaledCancel: a stage out of order");
            return refusal;
        }
        let tail = journal.tail();
        journal
            .compare_and_append(tail, &R::cancel(&self.name, asked))
            .map_err(CancelError::Journal)?;
        *stage = Some(asked);
        Ok(asked)
    }
}

impl<R> JournaledCancel<R> {
    /// The stage the records reach, or `None` when no stop was asked for.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::JournaledCancel;
    ///
    /// fn asked<R>(run: &JournaledCancel<R>) -> bool {
    ///     run.stage().is_some()
    /// }
    /// ```
    #[must_use]
    pub fn stage(&self) -> Option<CancelStage> {
        *locked(&self.stage)
    }

    /// The token this stop fires, for a scope with its own clock or trail.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::JournaledCancel;
    ///
    /// fn fired<R>(run: &JournaledCancel<R>) -> bool {
    ///     run.token().is_cancelled()
    /// }
    /// ```
    #[must_use]
    pub const fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// A root scope for `tenant` that this stop stops.
    ///
    /// # Examples
    ///
    /// ```
    /// use lgwks_bot::script::{JournaledCancel, Scope, Tenant};
    ///
    /// fn root<R>(run: &JournaledCancel<R>, tenant: Tenant) -> Scope {
    ///     run.scope(tenant)
    /// }
    /// ```
    #[must_use]
    pub fn scope(&self, tenant: Tenant) -> Scope {
        Scope::with_token(tenant, self.token.clone())
    }
}

impl<R> fmt::Debug for JournaledCancel<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JournaledCancel")
            .field("name", &self.name)
            .field("stage", &self.stage())
            .finish_non_exhaustive()
    }
}
