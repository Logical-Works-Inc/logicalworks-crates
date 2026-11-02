//! The journal half of sealed records: what a sealed read shows and what a
//! shred did.
//!
//! The methods live on [`FileJournal`](crate::journal::FileJournal) beside
//! the write path they seal — [`FileJournal::open_sealed`](crate::journal::FileJournal::open_sealed),
//! [`FileJournal::compare_and_append_sealed`](crate::journal::FileJournal::compare_and_append_sealed),
//! [`FileJournal::sealed_entries`](crate::journal::FileJournal::sealed_entries),
//! [`FileJournal::shred_sealed`](crate::journal::FileJournal::shred_sealed),
//! [`FileJournal::erase_sealed_tenant`](crate::journal::FileJournal::erase_sealed_tenant),
//! [`FileJournal::sweep_sealed`](crate::journal::FileJournal::sweep_sealed)
//! and [`FileJournal::verify_sealed_chain`](crate::journal::FileJournal::verify_sealed_chain) —
//! because they reach the handle's own fence, ladder and chain. This module
//! holds the two types those methods answer in, so a caller names one path
//! for both.

use crate::journal::{EffectEvent, JournalError, JournalPosition};

/// What one sealed journal frame reads as: the live event, or the tombstone
/// of a shredded record.
///
/// A shredded record keeps its chain position — the bytes still verify — but
/// carries no event, so it reads as erased rather than as its value and never
/// as absent.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum SealedJournalRead {
    /// The record is live: its position and its event, opened under its own
    /// data key.
    Live {
        /// Where the record sits in the chain.
        position: JournalPosition,
        /// The fact that was appended. Boxed because an event is two hundred
        /// bytes and the erased arm is one word.
        event: Box<EffectEvent>,
    },
    /// The record was shredded: its key destroyed, its bytes unreadable, its
    /// place in the chain still verified.
    Erased {
        /// The chain sequence of the shredded record.
        sequence: u64,
    },
}

/// What a shred did: shredded now, or already a tombstone.
///
/// Either way the record reads as erased afterwards, which is what makes the
/// second shred a no-op rather than a second destruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ShredOutcome {
    /// The key was destroyed and the tombstone committed by this call.
    Shredded {
        /// The chain sequence of the shredded record.
        sequence: u64,
    },
    /// The record was already a tombstone: nothing was destroyed and nothing
    /// was written.
    AlreadyErased {
        /// The chain sequence of the shredded record.
        sequence: u64,
    },
}

/// Map a provider refusal the write path met onto the journal's vocabulary.
///
/// Destroyed is unreachable here — the write path mints a fresh key per
/// record, and a fresh key is never destroyed — so every refusal is the
/// store's own error: the record was not written and the journal is
/// unchanged.
pub(crate) fn write_refusal(source: crate::sealed::keys::KeyError) -> JournalError {
    JournalError::Storage(std::io::Error::other(source.to_string()))
}
