//! The record a run store archives and how it is framed. The replay that
//! rebuilds the index from the file is the child module [`replay`].

#[cfg(feature = "sealed")]
use std::sync::Arc;
#[cfg(feature = "sealed")]
use std::time::Duration;

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::wire::{WireError, to_bytes};

use crate::effect::RunId;
use crate::journal::frame::{self, SaturatingFrom};
#[cfg(feature = "sealed")]
use crate::sealed::envelope::{NONCE_BYTES, seal as envelope_seal};
#[cfg(feature = "sealed")]
use crate::sealed::keys::{KeyId, KeyProvider};
#[cfg(feature = "sealed")]
use crate::sealed::record::encode_record;

#[cfg(feature = "sealed")]
use super::SealedWrite;
use super::{
    DefinitionIdentity, Index, MAX_RECORD_BYTES, MAX_STORE_BYTES, StoreError, StoreLimitKind,
};
use crate::script::approval::{RecordClass, classify_record};

mod replay;
pub(super) use replay::replay;
#[cfg(feature = "sealed")]
pub(super) use replay::replay_sealed;

/// A store refusal as the device error the owner's one reply channel carries.
///
/// One conversion rather than a `map_err` per arm, because every `?` on a store
/// refusal wraps the same fact: this store said no. The refusal travels as the
/// error's source, so its own `Display` names the ceiling or the tenant.
pub(super) fn refusal(cause: StoreError) -> std::io::Error {
    std::io::Error::other(cause)
}

/// The chain head the next frame in this file follows.
///
/// The index carries the tail head beside the committed length, because the head
/// cannot be recomputed from the index: a frame's head is over the *archived*
/// record, and re-archiving a value to hash it would be a second encoding of the
/// same bytes with its own chance of disagreeing with what is on the disk.
pub(super) fn tail_head(index: &Index) -> Digest {
    index.tail
}

/// The record as it is archived and as it is checked.
#[derive(
    Debug, Clone, lgwks_std::wire::Archive, lgwks_std::wire::Serialize, lgwks_std::wire::Deserialize,
)]
#[rkyv(
    attr(non_exhaustive),
    crate = lgwks_std::wire::rkyv,
    compare(PartialEq),
    derive(Debug)
)]
pub(super) struct Stored {
    /// The run this record belongs to.
    #[rkyv(attr(doc = "The run this record belongs to."))]
    pub(super) run: RunId,
    /// The step key's digest.
    #[rkyv(attr(doc = "The step key's digest."))]
    pub(super) key: Vec<u8>,
    /// The tenant that minted the run.
    #[rkyv(attr(doc = "The tenant that minted the run."))]
    pub(super) tenant: String,
    /// The step's path, for attribution.
    #[rkyv(attr(doc = "The step's path, for attribution."))]
    pub(super) path: String,
    /// The definition the record was written under, as its fields.
    ///
    /// Archived as fields rather than as one digest because a drift refusal has
    /// to *name* the two sides: a head over the digest could detect the
    /// disagreement but could not report which axis it was, and the fields are
    /// what a caller reads to decide whether it needs a new run or a migration.
    #[rkyv(attr(doc = "The definition the record was written under, as its fields."))]
    pub(super) definition: StoredDefinition,
    /// The archived value the step returned.
    #[rkyv(attr(doc = "The archived value the step returned."))]
    pub(super) value: Vec<u8>,
}

/// A [`DefinitionIdentity`] in the shape a record archives.
///
/// Named rather than archived directly so the record's on-disk shape is stated
/// once, in the store that writes it, and a future revision of the definition
/// vocabulary is a new struct here rather than a change to a type every consumer
/// of [`Drift`] is already matching.
#[derive(
    Debug, Clone, lgwks_std::wire::Archive, lgwks_std::wire::Serialize, lgwks_std::wire::Deserialize,
)]
#[rkyv(
    attr(non_exhaustive),
    crate = lgwks_std::wire::rkyv,
    compare(PartialEq),
    derive(Debug)
)]
pub(super) struct StoredDefinition {
    /// The task name.
    #[rkyv(attr(doc = "The task name."))]
    name: String,
    /// The declared definition revision.
    #[rkyv(attr(doc = "The declared definition revision."))]
    revision: u64,
    /// The input digest.
    #[rkyv(attr(doc = "The input digest."))]
    input: Vec<u8>,
    /// How many durable steps the definition declares.
    #[rkyv(attr(doc = "How many durable steps the definition declares."))]
    steps: u64,
    /// The declared durable-value schema id.
    #[rkyv(attr(doc = "The declared durable-value schema id."))]
    codec: String,
}

/// The 32 bytes a record archived, as the digest the identity carries back.
///
/// A fixed width rather than a slice, because a record whose input field is not
/// 32 bytes is a corrupt frame and the checked decoder is what says so; the
/// conversion cannot silently truncate, because `as` is forbidden in this
/// workspace and this is the honest alternative to it.
fn digest_from_record(bytes: &[u8]) -> Digest {
    let mut digest = [0u8; 32];
    digest
        .iter_mut()
        .zip(bytes)
        .for_each(|(slot, byte)| *slot = *byte);
    Digest::from_bytes(digest)
}

impl StoredDefinition {
    /// The record's archived form of a live identity.
    pub(super) fn of(identity: &DefinitionIdentity) -> Self {
        Self {
            name: identity.name().to_owned(),
            revision: identity.revision(),
            input: identity.input().as_bytes().to_vec(),
            steps: u64::saturating_from(identity.steps()),
            codec: identity.codec().to_owned(),
        }
    }

    /// The live identity this record was written under.
    pub(super) fn to_identity(&self) -> DefinitionIdentity {
        DefinitionIdentity::new(
            &self.name,
            self.revision,
            digest_from_record(&self.input),
            usize::saturating_from(self.steps),
        )
        .with_codec(&self.codec)
    }
}

impl Stored {
    /// This record's chain head over the head before it.
    ///
    /// Framed exactly as the step key is framed, so a record's head cannot be
    /// confused with the digest of the bytes it holds: the run, the key, the
    /// tenant, the path and every definition field are length-framed, and the
    /// value is framed last so no two different records hash the same byte
    /// string. The definition's own bytes go in through the identity's framing
    /// so a head over the record commits to the identity the same way the
    /// identity's digest commits to it.
    pub(super) fn head_from(&self, previous: &Digest, _archived: &[u8]) -> Digest {
        let mut hasher = Hasher::new();
        hasher.write_framed(previous.as_bytes());
        hasher.write_framed(&self.run_key_bytes());
        hasher.write_framed(&self.key);
        hasher.write_framed(self.tenant.as_bytes());
        hasher.write_framed(self.path.as_bytes());
        hasher.write_framed(self.definition.name.as_bytes());
        hasher.write_framed(&self.definition.revision.to_le_bytes());
        hasher.write_framed(&self.definition.input);
        hasher.write_framed(&self.definition.steps.to_le_bytes());
        hasher.write_framed(self.definition.codec.as_bytes());
        hasher.write_framed(&self.value);
        hasher.finalize()
    }

    /// The run id's 16 bytes, as a field the framing can hold.
    ///
    /// The hex spelling rather than the big-endian integer, because
    /// `RunId::to_hex` is the one conversion this crate defines for the role
    /// and a second one here would be a second definition to keep in step.
    fn run_key_bytes(&self) -> Vec<u8> {
        self.run.id().to_hex().into_bytes()
    }
}

/// The frame for one record: length, payload, head.
///
/// The head is stored rather than recomputed on faith. That is what separates a
/// torn tail (dropped, because a write leaves a prefix and a prefix has no head)
/// from a frame that does not follow (refused, because those bytes were
/// acknowledged).
pub(super) fn frame(stored: &Stored, previous: &Digest) -> Result<(Vec<u8>, Digest), StoreError> {
    // Archive, bound, chain and lay out — the same step the effect journal takes.
    // Only the record's own archiving and its step-chaining head are this store's.
    frame::frame_record(
        stored,
        previous,
        MAX_RECORD_BYTES,
        |stored| {
            to_bytes::<WireError>(stored)
                .map(|bytes| bytes.as_ref().to_vec())
                .map_err(|cause| StoreError::Encoding { cause })
        },
        Stored::head_from,
        record_too_large,
    )
}

/// The refusal for a store that has no room left, named by what it would have
/// needed.
///
/// One constructor because both call sites discovered the same fact by different
/// routes — an addition that overflowed and an addition that came out past the
/// ceiling — and a literal per site would be two chances to name a different bound
/// for the same ceiling.
pub(super) fn store_full(requested: u64) -> StoreError {
    StoreError::Limit {
        kind: StoreLimitKind::StoreBytes,
        requested,
        limit: MAX_STORE_BYTES,
    }
}

/// The refusal for a record whose archived bytes are past this store's ceiling.
///
/// One constructor, because the message and the two numbers are the same fact
/// whichever call site discovers it, and a literal per site would be two chances to
/// name a different bound for the same ceiling.
fn record_too_large(len: usize) -> StoreError {
    StoreError::Limit {
        kind: StoreLimitKind::RecordBytes,
        requested: u64::saturating_from(len),
        limit: u64::saturating_from(MAX_RECORD_BYTES),
    }
}

/// Fold one record's approval state into its tenant's pending count.
///
/// Pending parks increment their tenant; decisions decrement, saturating, so
/// a decision that arrives with no park counted — an app park recorded
/// elsewhere, a torn write that never folded — cannot borrow below zero.
/// Anything else leaves the count alone: the ceiling is over approvals, not
/// over records.
pub(super) fn count_approval(index: &mut Index, tenant: &str, bytes: &[u8]) {
    match classify_record(bytes) {
        RecordClass::Pending(_) => {
            let held = index.pending.entry(tenant.to_owned()).or_insert(0);
            *held = held.saturating_add(1);
        }
        RecordClass::Decision(_) => {
            if let Some(held) = index.pending.get_mut(tenant) {
                *held = held.saturating_sub(1);
            }
        }
        RecordClass::Other => {}
    }
}

/// Lowercase hex, for a digest used as a map key.
///
/// `lgwks_std::hash::Digest::to_hex` takes a `Digest`, and this is the same 32
/// bytes before the head has been computed; formatting them here rather than
/// reaching into the digest type keeps the key exactly the bytes on the disk.
pub(super) fn hex_of(bytes: &[u8]) -> String {
    let digit = |nibble: u8| match nibble {
        0..=9 => char::from(b'0'.saturating_add(nibble)),
        _ => char::from(b'a'.saturating_add(nibble.saturating_sub(10))),
    };
    bytes
        .iter()
        .flat_map(|byte| [digit(byte >> 4), digit(byte & 0x0f)])
        .collect()
}

/// The BLAKE3 of a plaintext step value: what the duplicate check compares
/// for a sealed step, so two identical appends still answer `AlreadyRecorded`
/// without the index holding any secret.
#[cfg(feature = "sealed")]
pub(super) fn value_digest(value: &[u8]) -> [u8; 32] {
    let mut hasher = Hasher::new();
    hasher.update(value);
    *hasher.finalize().as_bytes()
}

/// What a sealed frame the write path just framed carries for the fold.
#[cfg(feature = "sealed")]
pub(super) struct SealedFramed {
    /// The data key's identity.
    pub(super) key_id: KeyId,
    /// The wrapped data key, as the frame will store it.
    pub(super) wrapped: Vec<u8>,
    /// The seal nonce, as the frame will store it.
    pub(super) nonce: [u8; NONCE_BYTES],
    /// The ciphertext, as the frame will store it.
    pub(super) ciphertext: Vec<u8>,
    /// The BLAKE3 of the plaintext value, for the duplicate check.
    pub(super) digest: [u8; 32],
    /// When the record expires, as the frame will store it.
    pub(super) expires_at: Option<Duration>,
}

/// Destroy the orphan key `id` minted for an append that then refused:
/// a key no frame names must not outlive the call that minted it.
#[cfg(feature = "sealed")]
fn abandon_sealed(provider: &Arc<dyn KeyProvider>, id: &KeyId) {
    if let Err(error) = provider.destroy(id) {
        lgwks_std::trace::debug!(error = ?error, "sealed-frame: the orphan key could not be destroyed");
    }
}

/// The plaintext routing tag of a sealed step: the run and step hex beside
/// a zero byte, covered by the chain head with the ciphertext, so a sealed
/// frame names its own slot without the key.
#[cfg(feature = "sealed")]
pub(super) fn runstore_bind(run_hex: &str, key_hex: &str) -> Vec<u8> {
    [run_hex.as_bytes(), &[0], key_hex.as_bytes()].concat()
}

/// Split a routing tag into its run and step hex, or `None` for a shape this store never wrote.
#[cfg(feature = "sealed")]
pub(super) fn parse_runstore_bind(bind: &[u8]) -> Option<(String, String)> {
    match std::str::from_utf8(bind) {
        Ok(text) => text
            .split_once('\0')
            .map(|(run, key)| (run.to_owned(), key.to_owned())),
        Err(error) => {
            lgwks_std::trace::debug!(%error, "parse_runstore_bind: a tag is not text");
            None
        }
    }
}

/// Frame one step record sealed: burn nothing here — the duplicate check
/// already ran on the index — mint a fresh key, seal, wrap, and frame
/// through the shared grammar with the chain over the sealed bytes.
///
/// Every failure after the provision destroys the orphan key, so a refused
/// append leaves no managed key behind.
///
/// # Errors
///
/// [`StoreError::SealedKey`] when the provider refuses; [`StoreError::Limit`]
/// when the sealed frame exceeds the ceiling. Nothing is written by any of
/// them.
#[cfg(feature = "sealed")]
pub(super) fn frame_sealed(
    stored: &Stored,
    previous: &Digest,
    write: &SealedWrite,
) -> Result<(Vec<u8>, Digest, SealedFramed), StoreError> {
    let digest = value_digest(&stored.value);
    let archived = to_bytes::<WireError>(stored).map_err(|cause| StoreError::Encoding { cause })?;
    let archived = archived.as_ref().to_vec();
    let (key_id, data_key) = write.provider.provision(&stored.tenant).map_err(|error| {
        lgwks_std::trace::debug!(error = ?error, "frame_sealed: the provider refused to mint a key");
        StoreError::SealedKey { source: error }
    })?;
    let destroy_orphan = |provider: &Arc<dyn KeyProvider>, id: &KeyId| {
        abandon_sealed(provider, id);
    };
    let nonce: [u8; NONCE_BYTES] = match lgwks_std::random::bytes::<NONCE_BYTES>() {
        Ok(nonce) => nonce,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: no entropy for the seal nonce");
            destroy_orphan(&write.provider, &key_id);
            return Err(StoreError::SealedKey {
                source: crate::sealed::keys::KeyError::Transient {
                    reason: "no entropy for the seal nonce",
                },
            });
        }
    };
    let ciphertext = match envelope_seal(&data_key, &nonce, &archived) {
        Ok(ciphertext) => ciphertext,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: the seal step refused");
            destroy_orphan(&write.provider, &key_id);
            return Err(StoreError::Storage {
                cause: std::io::Error::other(error.to_string()),
            });
        }
    };
    drop(data_key);
    let wrapped = match write.provider.wrap_key(&key_id) {
        Ok(wrapped) => wrapped,
        Err(error) => {
            lgwks_std::trace::debug!(error = ?error, "frame_sealed: the wrap step refused");
            destroy_orphan(&write.provider, &key_id);
            return Err(StoreError::SealedKey { source: error });
        }
    };
    let bind = runstore_bind(&stored.run.id().to_hex(), &hex_of(&stored.key));
    let payload = encode_record(
        stored.tenant.as_str(),
        &key_id.id(),
        &wrapped,
        &nonce,
        &ciphertext,
        write.expires_at,
        &bind,
    );
    let (framed, head) = frame::frame_record(
        &payload,
        previous,
        MAX_RECORD_BYTES,
        |bytes| Ok(bytes.clone()),
        |_, prior, sealed| crate::journal::chain_over_bytes(prior, sealed),
        record_too_large,
    )
    .inspect_err(|_| {
        destroy_orphan(&write.provider, &key_id);
    })?;
    Ok((
        framed,
        head,
        SealedFramed {
            key_id,
            wrapped,
            nonce,
            ciphertext,
            digest,
            expires_at: write.expires_at,
        },
    ))
}
