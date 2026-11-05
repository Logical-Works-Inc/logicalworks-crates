//! A COSE_Sign1 receipt (RFC 9052) sealed with the estate's ed25519 signer.
//!
//! `sim <first> <count>` replays seeded tenants instead: a saboteur flips and
//! truncates sealed bytes, a neighbour opens another tenant's receipt, and a
//! double-clicker seals the same receipt twice.

#[path = "support/sim.rs"]
mod sim;

use lgwks_deps::coset::{CborSerializable, CoseSign1, CoseSign1Builder, HeaderBuilder, iana};
use lgwks_std::seeded::Seeded;
use lgwks_std::sign;

/// The domain the receipts are sealed under.
const TAG: &[u8] = b"lgwks/cose-sign1/v1";

/// The largest receipt a seeded run seals.
const SIM_MAX_PAYLOAD: u64 = 8_192;

fn main() {
    let passed = match sim::seeds() {
        Some(range) => simulate(range),
        None => journey(),
    };
    if !passed {
        std::process::exit(1);
    }
}

fn open(public: &sign::PublicKey, aad: &[u8], read: &CoseSign1) -> bool {
    read.verify_signature(aad, |signature, to_be_signed| {
        match <[u8; 64]>::try_from(signature) {
            Ok(raw) => {
                sign::verify_strict(public, TAG, to_be_signed, &sign::Signature::from_bytes(raw))
            }
            Err(short) => {
                lgwks_std::trace::debug!(error = %short, "cose: a signature that is not 64 bytes");
                Err(sign::VerifyError::Mismatch)
            }
        }
    })
    .is_ok()
}

/// Seal `payload` under `secret`, bound to `aad`, as untagged COSE_Sign1 bytes.
fn seal(secret: &sign::SecretKey, aad: &[u8], payload: Vec<u8>) -> Option<Vec<u8>> {
    let protected = HeaderBuilder::new()
        .algorithm(iana::Algorithm::EdDSA)
        .key_id(secret.public_key().to_bytes().to_vec())
        .build();
    CoseSign1Builder::new()
        .protected(protected)
        .payload(payload)
        .create_signature(aad, |to_be_signed| {
            sign::sign(secret, TAG, to_be_signed).to_bytes().to_vec()
        })
        .build()
        .to_vec()
        .ok()
}

/// Whether `bytes` decode to an envelope that opens under `public` and `aad`;
/// bytes that do not decode open nothing.
fn opens(public: &sign::PublicKey, aad: &[u8], bytes: &[u8]) -> bool {
    match CoseSign1::from_slice(bytes) {
        Ok(read) => open(public, aad, &read),
        Err(_undecodable) => false,
    }
}

fn journey() -> bool {
    let Ok(secret) = sign::generate() else {
        return false;
    };
    let public = secret.public_key();
    let Ok(stranger) = sign::generate() else {
        return false;
    };
    for size in [0_usize, 1, 32, 4_096] {
        let payload = vec![7_u8; size];
        let Some(bytes) = seal(&secret, b"", payload.clone()) else {
            return false;
        };
        let Ok(read) = CoseSign1::from_slice(&bytes) else {
            return false;
        };
        let intact =
            read.payload.as_deref() == Some(payload.as_slice()) && open(&public, b"", &read);
        let refused = !open(&public, b"other", &read) && !open(&stranger.public_key(), b"", &read);
        let truncated = bytes
            .get(..bytes.len().saturating_sub(1))
            .is_some_and(|short| CoseSign1::from_slice(short).is_err());
        if !(intact && refused && truncated) {
            return false;
        }
    }
    true
}

/// A tenant's key, from 32 seeded bytes: the same seed is the same tenant.
fn draw_tenant(rng: &mut Seeded) -> sign::SecretKey {
    let mut seed = [0_u8; 32];
    for chunk in seed.chunks_mut(8) {
        chunk.copy_from_slice(&rng.next_u64().to_le_bytes());
    }
    sign::SecretKey::from_seed(seed)
}

fn simulate(range: std::ops::Range<u64>) -> bool {
    let mut trace = sim::Trace::new();
    for seed in range {
        if one_seed(seed, &mut trace).is_none() {
            return false;
        }
    }
    trace.report()
}

/// One seeded pair of tenants. `None` names the first broken invariant.
fn one_seed(seed: u64, trace: &mut sim::Trace) -> Option<()> {
    let mut rng = Seeded::from_seed(seed);
    let alice = draw_tenant(&mut rng);
    let bob = draw_tenant(&mut rng);
    let size = usize::try_from(rng.below(SIM_MAX_PAYLOAD.saturating_add(1)).ok()?).ok()?;
    let payload: Vec<u8> = (0..size).map(|_| rng.next_u64().to_le_bytes()[0]).collect();
    let aad = seed.to_le_bytes();
    let sealed = seal(&alice, &aad, payload.clone())?;
    // The genuine receipt opens for its tenant and carries its payload.
    let read = CoseSign1::from_slice(&sealed).ok()?;
    if read.payload.as_deref() != Some(payload.as_slice())
        || !open(&alice.public_key(), &aad, &read)
    {
        return None;
    }
    // The double-clicker: Ed25519 signing is deterministic (RFC 8032 §5.1.6),
    // so sealing the same receipt twice is the same bytes, never a second one.
    if seal(&alice, &aad, payload)? != sealed {
        return None;
    }
    // The neighbour: Bob's key and Alice's receipt bound to another context.
    if opens(&bob.public_key(), &aad, &sealed) || opens(&alice.public_key(), b"", &sealed) {
        return None;
    }
    trace.record("neighbours_refused", seed);
    // The saboteur: any one flipped bit and any truncation is refused.
    for _ in 0..16 {
        let mut tampered = sealed.clone();
        let at = rng.index(tampered.len()).ok()?;
        let bit = u32::try_from(rng.below(8).ok()?).ok()?;
        let byte = tampered.get_mut(at)?;
        *byte ^= 1_u8.checked_shl(bit)?;
        if opens(&alice.public_key(), &aad, &tampered) {
            return None;
        }
        trace.record("flips_refused", u64::try_from(at).ok()?);
    }
    let cut = rng.index(sealed.len()).ok()?;
    if opens(&alice.public_key(), &aad, sealed.get(..cut)?) {
        return None;
    }
    trace.record("truncations_refused", u64::try_from(cut).ok()?);
    Some(())
}
