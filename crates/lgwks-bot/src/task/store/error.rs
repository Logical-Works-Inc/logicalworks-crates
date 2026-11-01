//! Why a run store refused: a ceiling, the device, the owning tenant, the
//! definition its records were written under, or the durability a step needed.

use std::fmt;
use std::sync::Arc;

use lgwks_std::wire::WireError;

use crate::journal::DurabilityPromise;
use crate::script::FlowError;

use super::Drift;

/// Which ceiling an append or a replay hit.
///
/// Named rather than collapsed into one "capacity exceeded" because the repair
/// differs per arm: one is a value too big, one is a run that grew, one is a
/// store that filled. A caller that cannot tell them apart raises every cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum StoreLimitKind {
    /// One archived record exceeded [`MAX_RECORD_BYTES`](super::MAX_RECORD_BYTES).
    RecordBytes,
    /// The run already holds [`MAX_RECORDS_PER_RUN`](super::MAX_RECORDS_PER_RUN)
    /// records.
    Records,
    /// The store file would exceed [`MAX_STORE_BYTES`](super::MAX_STORE_BYTES).
    StoreBytes,
    /// The tenant already holds [`MAX_PENDING_PER_TENANT`](crate::script::MAX_PENDING_PER_TENANT)
    /// undecided approvals.
    PendingApprovals,
}

/// Why a record could not be read, written, or resumed.
///
/// A read failure is never absence (INV-BOT-7): every arm here is an error the
/// caller must see, never an empty index that would cause a finished step to
/// re-run as though it had never run.
#[derive(Debug)]
#[non_exhaustive]
pub enum StoreError {
    /// A declared ceiling refused the write, or the replay. The store is
    /// unchanged: a refusal is not a compaction.
    Limit {
        /// Which ceiling.
        kind: StoreLimitKind,
        /// What would have been needed.
        requested: u64,
        /// The ceiling.
        limit: u64,
    },
    /// The store file could not be opened, read or written.
    Storage {
        /// What the device said.
        cause: std::io::Error,
    },
    /// The file exists but is not a run store, or its header is short.
    NotAStore,
    /// The file *is* a run store, in a format this build does not read.
    ///
    /// Distinct from [`NotAStore`][Self::NotAStore] because the two say opposite
    /// things about what is on the disk: one says these bytes were never this
    /// store's, the other says they were, and were written by a version of this
    /// crate whose records mean something this build cannot reconstruct. A
    /// pre-version store holds records with no
    /// [`DefinitionIdentity`](crate::task::DefinitionIdentity) in them, and the only
    /// ways to read those are to invent an identity they never carried — which
    /// makes every pre-version resume look exactly compatible — or to discard
    /// evidence a running system is relying on. So the store is refused with both
    /// versions named, and the caller is told which this build wants rather than
    /// being handed a corrupt-store error that says nothing about either.
    ///
    /// This is a breaking change for every existing run store, and it is
    /// deliberately not softened into one: the format has never shipped a version
    /// that could lose a record, so there is nothing to convert, and a deployment
    /// that needs its records keeps its own copy and re-runs.
    FormatVersion {
        /// The version byte the file declares.
        found: u8,
        /// The version this build reads.
        expected: u8,
    },
    /// A committed frame does not follow from the records before it. Those
    /// bytes are acknowledged evidence and are refused, never trimmed.
    Corrupt {
        /// The frame's index, from zero.
        at: u64,
    },
    /// The value could not be archived.
    Encoding {
        /// What the codec said.
        cause: WireError,
    },
    /// The run id belongs to a tenant other than the one resuming it.
    ///
    /// A refusal rather than an absence: the run *is* in this store, and reading
    /// its records for the wrong tenant is a cross-tenant read dressed up as a
    /// miss.
    ForeignTenant {
        /// The tenant that minted the run.
        owner: String,
        /// The tenant that asked to resume it.
        asked: String,
    },
    /// The run id names no run this store holds, so it cannot be attributed to
    /// any tenant here.
    ///
    /// Distinct from [`ForeignTenant`][Self::ForeignTenant] because the two say
    /// opposite things: one says the run exists and belongs elsewhere, the other
    /// says this store has never seen it. A resume in either case is refused —
    /// recording a run this host cannot attribute would put one tenant's work
    /// under another's name.
    UnknownRun {
        /// The run id that was asked for.
        run: String,
        /// The store that could not attribute it.
        tenant: String,
    },
    /// The run's records were written under a different definition identity, so
    /// replaying them would answer a question this build never asked.
    ///
    /// Refused before any step runs and with nothing written, because the two
    /// available answers are both worse: replaying is returning a value the
    /// current definition never produced, and re-running is repeating effects a
    /// previous attempt already performed. Which axis disagrees is named, so the
    /// caller knows whether it needs a new run, a decision, a migration or an
    /// edit.
    Incompatible {
        /// The run whose records disagree.
        run: String,
        /// Which axis, and what each side holds.
        drift: Drift,
    },
    /// The installed store promises less than the step needs: an effect was about
    /// to leave the process over a store that does not survive its writer dying.
    ///
    /// Refused before the effect leaves, because after a crash such a store
    /// reports an empty run rather than an uncertain one, and a recovered run
    /// would send the effect again blind.
    PromiseUnmet {
        /// What the step needs.
        required: DurabilityPromise,
        /// What the installed store promises.
        offered: DurabilityPromise,
    },
}

impl StoreError {
    /// A storage refusal carrying the device's own error.
    ///
    /// A constructor rather than seven literals, because every `map_err` in this
    /// module wraps the same fact — the device said no — and a literal per call
    /// site would be seven chances to name a different variant for the same
    /// event.
    ///
    /// `pub(crate)` because the repair ledger beside this store reports device
    /// refusals in the same vocabulary: one error type for both file-backed
    /// stores means a caller handling a device refusal handles it once.
    pub(crate) fn storage(cause: std::io::Error) -> Self {
        Self::Storage { cause }
    }
}

impl fmt::Display for StoreError {
    /// The refusal, naming the ceiling, the device, or the owning tenant.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Limit {
                kind,
                requested,
                limit,
            } => {
                let what = match kind {
                    StoreLimitKind::RecordBytes => "one record's bytes",
                    StoreLimitKind::Records => "records in one run",
                    StoreLimitKind::StoreBytes => "bytes in one run's store",
                    StoreLimitKind::PendingApprovals => "undecided approvals for one tenant",
                };
                write!(
                    formatter,
                    "{what}: {requested} exceeds the ceiling of {limit}"
                )
            }
            Self::Storage { ref cause } => {
                write!(formatter, "the run store's device refused: {cause}")
            }
            Self::NotAStore => {
                formatter.write_str("this file is not a run store; refusing to read it as records")
            }
            Self::FormatVersion { found, expected } => write!(
                formatter,
                "this is a run store in format version {found}, and this build reads version \
                 {expected}; refusing to read records whose definition identity this version \
                 cannot reconstruct"
            ),
            Self::Corrupt { at } => write!(
                formatter,
                "run store frame {at} does not follow from the records before it; \
                 the bytes are refused, not trimmed"
            ),
            Self::Encoding { ref cause } => {
                write!(formatter, "the step's value could not be archived: {cause}")
            }
            Self::ForeignTenant {
                ref owner,
                ref asked,
            } => write!(
                formatter,
                "run belongs to tenant {owner:?}, not {asked:?}; refusing to read its records"
            ),
            Self::UnknownRun {
                ref run,
                ref tenant,
            } => write!(
                formatter,
                "no records for run {run} in tenant {tenant:?}'s store; refusing to resume a \
                 run this store cannot attribute to it"
            ),
            Self::Incompatible { ref run, ref drift } => write!(
                formatter,
                "run {run} was recorded under a different definition: {}; refusing to replay \
                 it under this one",
                drift
            ),
            Self::PromiseUnmet { required, offered } => write!(
                formatter,
                "this step hands an effect out of the process, which needs a store that \
                 promises {required:?}, and the installed store promises {offered:?}; \
                 refusing before the effect leaves"
            ),
        }
    }
}

impl std::error::Error for StoreError {
    /// The device's or the codec's own error.
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Storage { ref cause } => Some(cause),
            Self::Encoding { ref cause } => Some(cause),
            Self::Limit { .. }
            | Self::NotAStore
            | Self::FormatVersion { .. }
            | Self::Corrupt { .. }
            | Self::ForeignTenant { .. }
            | Self::UnknownRun { .. }
            | Self::Incompatible { .. }
            | Self::PromiseUnmet { .. } => None,
        }
    }
}

impl From<StoreError> for FlowError {
    /// A store refusal is permanent: repeating the step would ask the same store
    /// the same question and get the same refusal.
    ///
    /// Two arms and not one, because the refusal's *kind* is what the caller acts
    /// on. A drift is already the flow's own typed [`FlowError::Incompatible`]
    /// carrying its axis, so it stays that rather than being buried one level
    /// down; every other store refusal — an unreadable device, a ceiling, a
    /// foreign tenant — reaches the caller as `FlowError::Store` wrapping the
    /// store's own error, so a read failure is never reported as a definition
    /// drift and never as a rendered string (INV-BOT-7).
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::Incompatible { drift, .. } => Self::incompatible("", drift),
            other => Self::Store {
                at: Arc::from(""),
                source: Box::new(other),
            },
        }
    }
}
