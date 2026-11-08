//! Tenant names against a reference model of the rule. A tenant reaches file
//! paths, so `.` and `..` must be refused the way CWE-22 (path traversal) names.

use lgwks_std::seeded::Seeded;

use super::{Tenant, name_fault};
use crate::script::MAX_TENANT_BYTES;

/// The seeds the sweep replays.
const SWEEP_SEEDS: [u64; 4] = [1, 0x5EED, 0xDEAD_BEEF, u64::MAX];

/// Bytes the fool types: legal ones, dots weighted heavily, path and control bytes.
const ALPHABET: &[u8] = b"....aZ9-_:/\\\0 ";

/// The rule written apart from the code: legal bytes, bounded, not only dots.
fn model_accepts(name: &[u8]) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TENANT_BYTES
        && name
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(byte))
        && name.iter().any(|byte| *byte != b'.')
        && name != b"fan-out"
}

/// Catches: `.` or `..` accepted as a tenant, which a store joining the name
/// into a path would resolve outside its root.
#[test]
fn sim_tenant_weird_bytes_dot_paths_are_never_tenants() -> Result<(), String> {
    for dots in 1..=MAX_TENANT_BYTES {
        let name = ".".repeat(dots);
        if Tenant::new(&name).is_ok() {
            return Err(format!("{dots} dots became a tenant"));
        }
    }
    for seed in SWEEP_SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..4_096 {
            let length = stream.index(8).map_err(|error| error.to_string())?;
            let mut name = Vec::with_capacity(length);
            for _ in 0..length {
                let at = stream
                    .index(ALPHABET.len())
                    .map_err(|error| error.to_string())?;
                name.extend(ALPHABET.get(at).copied());
            }
            let text = String::from_utf8(name.clone()).map_err(|error| error.to_string())?;
            if Tenant::new(&text).is_ok() != model_accepts(&name) {
                return Err(format!("seed {seed:#x}: {text:?} disagrees with the model"));
            }
        }
    }
    Ok(())
}

/// Catches: the shared rule drifting, since a task name and a request key take
/// the same check. A dotted name with one other byte stays legal for all three.
#[test]
fn sim_tenant_wrong_order_a_dot_beside_a_letter_stays_legal() -> Result<(), String> {
    for name in [".a", "a.", "a..b", ".:", "..-"] {
        if name_fault(name, MAX_TENANT_BYTES).is_some() {
            return Err(format!("{name:?} must stay a legal name"));
        }
    }
    Ok(())
}
