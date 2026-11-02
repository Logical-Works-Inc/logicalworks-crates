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
//! holds the two types those methods answer in, so a caller names one path
//! for both.

use crate::effect::RunId;
use crate::script::{StepKey, run_store::StoredValue};

/// What one sealed step reads as: the live value, or the tombstone of a
/// shredded record.
///
/// There is no absent arm: an unknown step is [`StoreError::SealedUnknown`](crate::task::StoreError::SealedUnknown),
/// never a miss that would let a finished step run again.
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
