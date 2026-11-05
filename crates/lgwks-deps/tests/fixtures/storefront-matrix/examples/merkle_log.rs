//! An RFC 6962 receipt log: inclusion and consistency proofs hold, a forgery is refused.
//!
//! The tree is RFC 6962's; its hash is the estate's BLAKE3 (`lgwks_std::hash`)
//! behind the `digest` traits the crate is generic over. `sim <first> <count>`
//! replays seeded logs instead: every root is checked against a reference
//! Merkle Tree Hash computed from RFC 6962 §2.1 beside the crate, never against
//! the crate's own output.

#[path = "support/sim.rs"]
mod sim;

use lgwks_deps::storefront::ct_merkle::digest::consts::U32;
use lgwks_deps::storefront::ct_merkle::digest::{
    FixedOutput, HashMarker, Output, OutputSizeUser, Update,
};
use lgwks_deps::storefront::ct_merkle::mem_backed_tree::MemoryBackedTree;
use lgwks_std::hash::Hasher;
use lgwks_std::seeded::Seeded;

/// The estate's BLAKE3 as the `digest::Digest` the tree hashes with.
#[derive(Default)]
struct Blake3(Hasher);

impl Update for Blake3 {
    fn update(&mut self, data: &[u8]) {
        self.0.update(data);
    }
}

impl OutputSizeUser for Blake3 {
    type OutputSize = U32;
}

impl FixedOutput for Blake3 {
    fn finalize_into(self, out: &mut Output<Self>) {
        out.copy_from_slice(self.0.finalize().as_bytes());
    }
}

impl HashMarker for Blake3 {}

/// The log grows to this many entries; the consistency proof spans its first half.
const ENTRIES: usize = 1_024;

/// The largest log a seeded run grows: past one power of two, so the right
/// subtree of the reference split is never full.
const SIM_MAX_ENTRIES: u64 = 1_100;

fn main() {
    let passed = match sim::seeds() {
        Some(range) => simulate(range),
        None => journey(),
    };
    if !passed {
        std::process::exit(1);
    }
}

fn entry(index: usize) -> Vec<u8> {
    format!("receipt-{index}").into_bytes()
}

fn journey() -> bool {
    let half = ENTRIES / 2;
    let mut log = MemoryBackedTree::<Blake3, Vec<u8>>::new();
    for index in 0..half {
        log.push(entry(index));
    }
    let old_root = log.root();
    for index in half..ENTRIES {
        log.push(entry(index));
    }
    let root = log.root();
    if log.len() != ENTRIES as u64 {
        return false;
    }
    let last = ENTRIES.saturating_sub(1);
    for index in [0, 1, half.saturating_sub(1), half, last] {
        let proof = log.prove_inclusion(index);
        if root
            .verify_inclusion(&entry(index), index as u64, &proof)
            .is_err()
        {
            return false;
        }
        if root
            .verify_inclusion(&b"forged".to_vec(), index as u64, &proof)
            .is_ok()
        {
            return false;
        }
    }
    let consistency = log.prove_consistency(half);
    root.verify_consistency(&old_root, &consistency).is_ok()
}

/// RFC 6962 §2.1 with H = BLAKE3: MTH({}) = H(), MTH({d}) = H(0x00 || d), and
/// MTH(D[n]) = H(0x01 || MTH(D[0:k]) || MTH(D[k:n])) for the largest power of
/// two k below n. It hashes through `lgwks_std::hash` directly, not the adapter.
fn reference_root(leaves: &[Vec<u8>]) -> Option<Vec<u8>> {
    match leaves {
        [] => Some(lgwks_std::hash::blake3(&[]).as_bytes().to_vec()),
        [only] => {
            let mut hasher = Hasher::new();
            hasher.update(&[0_u8]).update(only);
            Some(hasher.finalize().as_bytes().to_vec())
        }
        _ => {
            let mut split = 1_usize;
            while split.saturating_mul(2) < leaves.len() {
                split = split.saturating_mul(2);
            }
            let (left, right) = leaves.split_at_checked(split)?;
            let (left, right) = (reference_root(left)?, reference_root(right)?);
            let mut hasher = Hasher::new();
            hasher.update(&[1_u8]).update(&left).update(&right);
            Some(hasher.finalize().as_bytes().to_vec())
        }
    }
}

/// The weird-bytes fool's entry: empty, one NUL, a run of 0xFF, invalid
/// UTF-8, or seeded noise, and sometimes the previous entry again.
fn draw_entry(rng: &mut Seeded, previous: Option<&Vec<u8>>) -> Option<Vec<u8>> {
    let shape = rng.below(6).ok()?;
    let entry = match (shape, previous) {
        (0, _) => Vec::new(),
        (1, _) => vec![0_u8],
        (2, _) => vec![0xFF_u8; 64],
        (3, _) => vec![0xC0_u8, 0x80, 0xED, 0xA0, 0x80],
        (4, Some(again)) => again.clone(),
        _ => {
            let len = rng.below(300).ok()?;
            (0..len).map(|_| rng.next_u64().to_le_bytes()[0]).collect()
        }
    };
    Some(entry)
}

/// The giant-and-empty fool's log size: empty, one, a power of two and its
/// neighbours, or anything up to [`SIM_MAX_ENTRIES`].
fn draw_size(rng: &mut Seeded) -> Option<usize> {
    let size = match rng.below(4).ok()? {
        0 => 0,
        1 => 1,
        2 => {
            let power = 1_u64.checked_shl(u32::try_from(rng.below(11).ok()?).ok()?)?;
            power.saturating_add(rng.below(3).ok()?).saturating_sub(1)
        }
        _ => rng.below(SIM_MAX_ENTRIES).ok()?,
    };
    usize::try_from(size).ok()
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

/// One seeded log. `None` names the first broken invariant, after which the
/// whole run fails.
fn one_seed(seed: u64, trace: &mut sim::Trace) -> Option<()> {
    let mut rng = Seeded::from_seed(seed);
    let size = draw_size(&mut rng)?;
    let mut leaves: Vec<Vec<u8>> = Vec::with_capacity(size);
    let mut log = MemoryBackedTree::<Blake3, Vec<u8>>::new();
    // A prefix of at least one entry, so a consistency proof exists for it.
    let prefix = if size == 0 {
        0
    } else {
        rng.index(size).ok()?.saturating_add(1)
    };
    let mut old_root = None;
    for index in 0..size {
        let next = draw_entry(&mut rng, leaves.last())?;
        log.push(next.clone());
        leaves.push(next);
        if index.saturating_add(1) == prefix {
            old_root = Some((log.root(), reference_root(&leaves)?));
        }
    }
    let root = log.root();
    let reference = reference_root(&leaves)?;
    if root.as_bytes().as_slice() != reference.as_slice() || root.num_leaves() != log.len() {
        return None;
    }
    trace.record("roots", u64::from(reference.first().copied()?));
    if size == 0 {
        return Some(());
    }
    // The saboteur: every proved entry verifies, the same proof refuses a
    // flipped byte and a different entry, at the index it was made for.
    for _ in 0..4 {
        let index = rng.index(size).ok()?;
        let proof = log.prove_inclusion(index);
        let genuine = leaves.get(index)?;
        let at = u64::try_from(index).ok()?;
        root.verify_inclusion(genuine, at, &proof).ok()?;
        let mut flipped = genuine.clone();
        match flipped.first_mut() {
            Some(byte) => *byte ^= 0x01,
            None => flipped.push(0),
        }
        if root.verify_inclusion(&flipped, at, &proof).is_ok() {
            return None;
        }
        trace.record("forgeries_refused", at);
    }
    // The wrong-order fool's prefix: the old root is the reference MTH of the
    // prefix, and the new log proves it is a prefix of itself.
    let (old, old_reference) = old_root?;
    if old.as_bytes().as_slice() != old_reference.as_slice() {
        return None;
    }
    let additions = size.checked_sub(prefix)?;
    let consistency = log.prove_consistency(additions);
    root.verify_consistency(&old, &consistency).ok()?;
    trace.record("consistent_prefixes", u64::try_from(prefix).ok()?);
    Some(())
}
