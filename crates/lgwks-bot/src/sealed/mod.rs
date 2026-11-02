//! Sealed records at rest: every journal and run-store record written under the
//! `sealed` feature is encrypted, shredded by key destruction, and erased by
//! tenant, by record, or by TTL.
//!
//! | Concern | Where it lives |
//! |---|---|
//! | key wrapping, rotation and destruction | [`keys`] |
//! | the AEAD envelope over one record | [`envelope`] |
//! | the journal's sealed reads and shreds | [`journal`] |
//! | the run store's sealed reads and shreds | [`runstore`] |
//! | the owned TTL sweep task | [`sweep`] |
//! | how a read folds a provider refusal | `fold` (crate-internal) |
//! | one record's bytes | `record` (crate-internal) |
//!
//! # The promise
//!
//! A sealed record is unreadable without its data key, and each record holds a
//! data key of its own: [`KeyProvider::provision`] mints a fresh 32-byte
//! key per record, never shared across records or tenants. The archived bytes
//! go through the envelope with that key, and the frame's chain head covers
//! the ciphertext together with the key identity, so a shredded record still
//! verifies as present-and-erased through the stores' existing chain checks.
//!
//! # Shredding is key destruction
//!
//! Shredding destroys the record's data key in the provider first — durably,
//! through the provider's destruction log — and appends a tombstone frame to
//! the same file second. Either half surviving a crash leaves the record live
//! or erased, never readable-without-its-managed-key: a destroyed key folds
//! to erased even when the tombstone frame never landed, and a tombstone
//! frame reads as erased even when the provider forgot the destruction. The
//! bytes are never rewritten (NIST SP 800-88 crypto-erase).
//!
//! # Reads fold, they do not guess
//!
//! A destroyed key folds to erased; a missing key or any other provider
//! refusal folds to an error; a transient provider fault propagates as itself,
//! so it neither resurrects the record (returning bytes) nor drops it
//! (reporting it erased). The fold is one function in `fold`, called on every
//! sealed read in both stores.
//!
//! # Erasure
//!
//! Shredding erases one record, tenant erasure shreds every live record of
//! one tenant (GDPR Art. 17), and [`Sweeper`] shreds what a TTL
//! passed, driven by the injected [`Clock`]. The
//! background driver owns its one task in a
//! [`JoinSet`] listening to one
//! [`CancellationToken`].
//!
//! [`keys`]: crate::sealed::keys
//! [`envelope`]: crate::sealed::envelope
//! [`journal`]: crate::sealed::journal
//! [`runstore`]: crate::sealed::runstore
//! [`sweep`]: crate::sealed::sweep
//! [`KeyProvider::provision`]: crate::sealed::keys::KeyProvider::provision
//! [`Sweeper`]: crate::sealed::sweep::Sweeper
//! [`Clock`]: crate::clock::Clock
//! [`JoinSet`]: crate::rt::task::JoinSet
//! [`CancellationToken`]: crate::rt::sync::CancellationToken

/// The AEAD envelope over one sealed record: the zeroizing key handle and the
/// seal/open step over ciphertext.
pub mod envelope;

/// Wrapped data keys: the `KeyProvider` port, its typed
/// refusals, key identities, and the software provider.
pub mod keys;

/// The journal half of sealed records: what a sealed read shows and what a
/// shred did.
pub mod journal;

/// The run-store half of sealed records: what a sealed step read shows and
/// what a shred did.
pub mod runstore;

/// The owned TTL driver: the `Sweeper` that sweeps expired records on the
/// blocking pool under a cancellation token.
pub mod sweep;

/// How a sealed read folds a provider refusal: destroyed to a tombstone,
/// missing and any other refusal to an error, transient to a retryable error
/// that neither resurrects nor drops the record. Crate-internal because the
/// fold is a step of a sealed read, not a second reader callers could reach
/// past it.
pub(crate) mod fold;

/// One sealed record's bytes: the frame payload codec. Crate-internal like
/// the fold: the codec is a step of the sealed frame, not a second reader.
pub(crate) mod record;
