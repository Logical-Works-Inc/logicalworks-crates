//! How a sealed read folds a provider refusal: destroyed to a tombstone,
//! missing and any other refusal to an error, transient to a retryable error
//! that neither resurrects nor drops the record.
//!
//! One function, called on every sealed read in both durable stores, so the
//! three dispositions cannot drift between the journal, the run store and the
//! sweep: a destroyed key means the record was shredded and reads as erased;
//! a missing key or any other provider refusal means the store cannot answer
//! and reads as an error; a transient refusal means the provider could not
//! answer *right now*, so it propagates as itself — returning bytes would
//! resurrect a record whose key may be gone, and reporting it erased would
//! drop a record whose key may be fine.
//!
//! Crate-internal because the fold is a step of a sealed read, not a second
//! reader callers could reach past it.

use super::envelope::DataKey;
use super::keys::KeyError;

/// What a sealed read shows once the provider has answered: the live key, or
/// the tombstone of a key the provider destroyed.
pub(crate) enum Unwrapped {
    /// The provider holds the key: the caller opens the envelope under it.
    Live(DataKey),
    /// The provider destroyed the key: the record reads as erased without
    /// touching the envelope.
    Erased,
}

/// Fold a provider answer into what the read shows.
///
/// `Err(KeyError::Destroyed)` folds to [`Unwrapped::Erased`]; every other
/// refusal, transient included, propagates as itself, so a transient fault
/// never folds either way and the caller retries rather than resurrects or
/// drops the record. The envelope step runs after this, only for a live key:
/// its refusal means the frame's bytes do not match their key, which is
/// corruption rather than erasure, and the caller reports it as its own.
pub(crate) fn fold_unwrap(answer: Result<DataKey, KeyError>) -> Result<Unwrapped, KeyError> {
    match answer {
        Ok(key) => Ok(Unwrapped::Live(key)),
        Err(KeyError::Destroyed { .. }) => {
            lgwks_std::trace::debug!("sealed-fold: a destroyed key folds to a tombstone");
            Ok(Unwrapped::Erased)
        }
        Err(KeyError::Transient { reason }) => {
            lgwks_std::trace::debug!(
                reason,
                "sealed-fold: a transient fault propagates without folding"
            );
            Err(KeyError::Transient { reason })
        }
        Err(other) => {
            lgwks_std::trace::debug!(error = ?other, "sealed-fold: a provider refusal folds to an error");
            Err(other)
        }
    }
}
