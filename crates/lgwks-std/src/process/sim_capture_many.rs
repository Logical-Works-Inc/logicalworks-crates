//! Seeded simulation and boundary tests for batch descendant capture (`capture_descendants_many`).

use std::io;

use super::capture::MAX_CAPTURED_DESCENDANTS;
use super::capture_many::{MAX_CAPTURE_ROOTS, capture_descendants_many, capture_many_from_pairs};
use crate::seeded::{SWEEP_SEEDS, Seeded};

/// Creates an `io::Error` with kind `Other`.
fn err_other(msg: &'static str) -> io::Error {
    io::Error::other(msg)
}

/// Ensure a boolean condition is true or return a descriptive static error.
fn ensure(condition: bool, message: &'static str) -> Result<(), io::Error> {
    if !condition {
        crate::trace::debug!(message, "assertion failed");
        return Err(err_other(message));
    }
    Ok(())
}

/// Ensure a boolean condition is true or return a descriptive dynamic error.
fn ensure_msg(condition: bool, message: String) -> Result<(), io::Error> {
    if !condition {
        crate::trace::debug!(%message, "assertion failed");
        return Err(io::Error::other(message));
    }
    Ok(())
}

/// Generate a pseudo-random `i32` bounded by `bound` (exclusive) using `Seeded`.
fn next_bounded_i32(rng: &mut Seeded, bound: i32) -> i32 {
    if bound <= 0 {
        return 0;
    }
    let mut raw = 0_i32;
    if let Ok(val) = i32::try_from(rng.next_u64() & 0x7fff_ffff) {
        raw = val;
    }
    let mut out = 0_i32;
    if let Some(rem) = raw.checked_rem(bound) {
        out = rem;
    }
    out
}

/// Test that empty root slices return an empty map immediately.
fn test_empty_roots() -> Result<(), io::Error> {
    let empty_from_pairs = capture_many_from_pairs(&[], &[])?;
    ensure(
        empty_from_pairs.is_empty(),
        "expected empty map for empty roots in capture_many_from_pairs",
    )?;

    let empty_with_pairs = capture_many_from_pairs(&[], &[(10, 1)])?;
    ensure(
        empty_with_pairs.is_empty(),
        "expected empty map for empty roots with non-empty pairs",
    )?;

    let empty_os = capture_descendants_many(&[])?;
    ensure(
        empty_os.is_empty(),
        "expected empty map for empty roots in capture_descendants_many",
    )?;

    Ok(())
}

/// Test validation of MAX_CAPTURE_ROOTS and non-positive process ids.
fn test_invalid_inputs() -> Result<(), io::Error> {
    let too_many = vec![1_i32; MAX_CAPTURE_ROOTS.saturating_add(1)];
    match capture_many_from_pairs(&too_many, &[]) {
        Err(err) => {
            ensure(
                err.kind() == io::ErrorKind::InvalidInput,
                "expected InvalidInput for roots exceeding MAX_CAPTURE_ROOTS in capture_many_from_pairs",
            )?;
        }
        Ok(_) => {
            crate::trace::debug!(
                "expected error for roots exceeding MAX_CAPTURE_ROOTS in capture_many_from_pairs"
            );
            return Err(err_other(
                "expected error for roots exceeding MAX_CAPTURE_ROOTS in capture_many_from_pairs",
            ));
        }
    }

    match capture_descendants_many(&too_many) {
        Err(err) => {
            ensure(
                err.kind() == io::ErrorKind::InvalidInput,
                "expected InvalidInput for roots exceeding MAX_CAPTURE_ROOTS in capture_descendants_many",
            )?;
        }
        Ok(_) => {
            crate::trace::debug!(
                "expected error for roots exceeding MAX_CAPTURE_ROOTS in capture_descendants_many"
            );
            return Err(err_other(
                "expected error for roots exceeding MAX_CAPTURE_ROOTS in capture_descendants_many",
            ));
        }
    }

    let bad_root_cases: &[&[i32]] = &[&[0], &[-1], &[10, 0, 20], &[5, -42]];
    for bad_roots in bad_root_cases {
        match capture_many_from_pairs(bad_roots, &[]) {
            Err(err) => {
                ensure(
                    err.kind() == io::ErrorKind::InvalidInput,
                    "expected InvalidInput for non-positive root in capture_many_from_pairs",
                )?;
            }
            Ok(_) => {
                crate::trace::debug!(
                    "expected error for non-positive root in capture_many_from_pairs"
                );
                return Err(err_other(
                    "expected error for non-positive root in capture_many_from_pairs",
                ));
            }
        }

        match capture_descendants_many(bad_roots) {
            Err(err) => {
                ensure(
                    err.kind() == io::ErrorKind::InvalidInput,
                    "expected InvalidInput for non-positive root in capture_descendants_many",
                )?;
            }
            Ok(_) => {
                crate::trace::debug!(
                    "expected error for non-positive root in capture_descendants_many"
                );
                return Err(err_other(
                    "expected error for non-positive root in capture_descendants_many",
                ));
            }
        }
    }

    Ok(())
}

/// Test performance and correctness with 10,000 roots over a frozen table.
fn test_10000_roots_performance_and_soundness() -> Result<(), io::Error> {
    let roots: Vec<i32> = (1..=10_000).collect();
    let mut pairs = Vec::new();
    for root_id in 1..=200_i32 {
        pairs.push((20_000_i32.saturating_add(root_id), root_id));
        pairs.push((30_000_i32.saturating_add(root_id), root_id));
        pairs.push((
            40_000_i32.saturating_add(root_id),
            30_000_i32.saturating_add(root_id),
        ));
    }

    let batch = capture_many_from_pairs(&roots, &pairs)?;
    ensure_msg(
        batch.len() == 10_000,
        format!("expected 10000 entries, got {}", batch.len()),
    )?;

    // Check first 50 roots (which have descendants).
    for test_root in 1..=50_i32 {
        let single = capture_many_from_pairs(&[test_root], &pairs)?;
        let single_set = single
            .get(&test_root)
            .ok_or_else(|| err_other("missing single set"))?;
        let batch_set = batch
            .get(&test_root)
            .ok_or_else(|| err_other("missing batch set"))?;
        ensure_msg(
            single_set == batch_set,
            format!("10k root mismatch for root {test_root}"),
        )?;
        ensure(
            batch_set.len() == 3,
            "expected exactly 3 descendants for root 1..=50",
        )?;
        ensure(
            !batch_set.is_truncated(),
            "expected non-truncated set for root 1..=50",
        )?;
    }

    // Spot-check some roots without descendants.
    for test_root in 500..=520_i32 {
        let batch_set = batch
            .get(&test_root)
            .ok_or_else(|| err_other("missing batch set"))?;
        ensure(
            batch_set.is_empty(),
            "expected empty descendant set for unbranched root",
        )?;
    }

    Ok(())
}

/// Test that overlapping parent/child roots both receive full descendant sets.
fn test_overlapping_roots() -> Result<(), io::Error> {
    let pairs = vec![(101, 100), (102, 100), (103, 101), (104, 101), (105, 104)];
    let roots = vec![100, 101, 104];
    let batch = capture_many_from_pairs(&roots, &pairs)?;

    for root_ref in &roots {
        let root_id = *root_ref;
        let single = capture_many_from_pairs(&[root_id], &pairs)?;
        let single_set = single
            .get(&root_id)
            .ok_or_else(|| err_other("missing root in single"))?;
        let batch_set = batch
            .get(&root_id)
            .ok_or_else(|| err_other("missing root in batch"))?;
        ensure_msg(
            single_set == batch_set,
            format!("overlapping root mismatch for root {root_id}"),
        )?;
    }

    let set_100 = batch
        .get(&100)
        .ok_or_else(|| err_other("missing root 100"))?;
    ensure_msg(
        set_100.len() == 5,
        format!("expected 5 descendants for root 100, got {}", set_100.len()),
    )?;
    ensure(set_100.contains(101), "root 100 must contain 101")?;
    ensure(set_100.contains(102), "root 100 must contain 102")?;
    ensure(set_100.contains(103), "root 100 must contain 103")?;
    ensure(set_100.contains(104), "root 100 must contain 104")?;
    ensure(set_100.contains(105), "root 100 must contain 105")?;

    let set_101 = batch
        .get(&101)
        .ok_or_else(|| err_other("missing root 101"))?;
    ensure_msg(
        set_101.len() == 3,
        format!("expected 3 descendants for root 101, got {}", set_101.len()),
    )?;
    ensure(set_101.contains(103), "root 101 must contain 103")?;
    ensure(set_101.contains(104), "root 101 must contain 104")?;
    ensure(set_101.contains(105), "root 101 must contain 105")?;

    let set_104 = batch
        .get(&104)
        .ok_or_else(|| err_other("missing root 104"))?;
    ensure_msg(
        set_104.len() == 1,
        format!("expected 1 descendant for root 104, got {}", set_104.len()),
    )?;
    ensure(set_104.contains(105), "root 104 must contain 105")?;

    // Reversed root order must produce identical output.
    let reversed_roots = vec![104, 101, 100];
    let reversed_batch = capture_many_from_pairs(&reversed_roots, &pairs)?;
    ensure(
        batch == reversed_batch,
        "root traversal order must not affect final descendant sets",
    )?;

    Ok(())
}

/// Test deterministic equivalence between batch and per-root traversals over seeded graphs.
fn test_seeded_multi_root_graphs_sweep() -> Result<(), io::Error> {
    for seed in SWEEP_SEEDS.iter().copied() {
        let mut rng = Seeded::from_seed(seed);
        let node_count = 100_i32;
        let base_pid = 1_000_i32;

        let mut pairs = Vec::new();
        // Generate a random forest where each node connects to an earlier node or has no parent.
        for offset in 1..node_count {
            let pid = base_pid.saturating_add(offset);
            let parent_offset = next_bounded_i32(&mut rng, offset);
            let parent_pid = base_pid.saturating_add(parent_offset);
            pairs.push((pid, parent_pid));
        }

        // Pick 15 random roots from the node set.
        let mut roots = Vec::new();
        for _ in 0..15_i32 {
            let offset = next_bounded_i32(&mut rng, node_count);
            let root = base_pid.saturating_add(offset);
            if !roots.contains(&root) {
                roots.push(root);
            }
        }

        let batch = capture_many_from_pairs(&roots, &pairs)?;
        for root_ref in &roots {
            let root = *root_ref;
            let single = capture_many_from_pairs(&[root], &pairs)?;
            let single_set = single
                .get(&root)
                .ok_or_else(|| err_other("missing root in single sweep"))?;
            let batch_set = batch
                .get(&root)
                .ok_or_else(|| err_other("missing root in batch sweep"))?;
            ensure_msg(
                single_set == batch_set,
                format!("seeded sweep mismatch for root {root} under seed {seed}"),
            )?;
        }
    }
    Ok(())
}

/// Test that batch traversal enforces MAX_CAPTURED_DESCENDANTS truncation.
fn test_max_captured_descendants_truncation() -> Result<(), io::Error> {
    let excess_count = 5_000_i32;
    let mut pairs = Vec::new();
    for child in 2..=excess_count.saturating_add(1) {
        pairs.push((child, 1));
    }

    let batch = capture_many_from_pairs(&[1], &pairs)?;
    let set = batch
        .get(&1)
        .ok_or_else(|| err_other("missing root 1 in batch"))?;
    ensure(
        set.is_truncated(),
        "expected set to be truncated when children exceed MAX_CAPTURED_DESCENDANTS",
    )?;
    ensure_msg(
        set.len() == MAX_CAPTURED_DESCENDANTS,
        format!(
            "expected len {}, got {}",
            MAX_CAPTURED_DESCENDANTS,
            set.len()
        ),
    )?;

    let single = capture_many_from_pairs(&[1], &pairs)?;
    let single_set = single
        .get(&1)
        .ok_or_else(|| err_other("missing root 1 in single"))?;
    ensure(
        set == single_set,
        "batch truncation must match single root truncation",
    )?;

    Ok(())
}

/// Test that batch traversal enforces MAX_CAPTURED_DEPTH truncation.
fn test_max_captured_depth_truncation() -> Result<(), io::Error> {
    let chain_len = 80_i32;
    let mut pairs = Vec::new();
    for idx in 1..=chain_len {
        pairs.push((idx.saturating_add(1), idx));
    }

    let batch = capture_many_from_pairs(&[1], &pairs)?;
    let set = batch
        .get(&1)
        .ok_or_else(|| err_other("missing root 1 in depth batch"))?;
    ensure(
        set.is_truncated(),
        "expected set to be truncated when depth exceeds MAX_CAPTURED_DEPTH",
    )?;
    ensure_msg(
        set.len() == 64,
        format!("expected depth truncated set len 64, got {}", set.len()),
    )?;

    let single = capture_many_from_pairs(&[1], &pairs)?;
    let single_set = single
        .get(&1)
        .ok_or_else(|| err_other("missing root 1 in depth single"))?;
    ensure(
        set == single_set,
        "batch depth truncation must match single depth truncation",
    )?;

    Ok(())
}

/// Test real OS capture for current process id.
fn test_real_os_capture() -> Result<(), io::Error> {
    let my_pid = i32::try_from(std::process::id())
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err.to_string()))?;
    let os_map = capture_descendants_many(&[my_pid])?;
    ensure(
        os_map.contains_key(&my_pid),
        "OS capture must contain caller pid",
    )?;
    let caller_set = os_map
        .get(&my_pid)
        .ok_or_else(|| err_other("missing caller pid entry"))?;
    ensure(
        !caller_set.is_truncated(),
        "caller descendant set should not be truncated",
    )?;

    // Also verify duplicate roots in slice.
    let dup_map = capture_descendants_many(&[my_pid, my_pid])?;
    ensure(
        dup_map.contains_key(&my_pid),
        "OS capture with duplicate roots must contain caller pid",
    )?;

    Ok(())
}

/// Verify batch descendant capture across seeded multi-root graphs, 10,000 roots,
/// overlapping parent/child roots, truncation bounds, validation, and real OS calls.
#[test]
fn sim_capture_many_hierarchical_sweep() -> Result<(), io::Error> {
    test_empty_roots()?;
    test_invalid_inputs()?;
    test_10000_roots_performance_and_soundness()?;
    test_overlapping_roots()?;
    test_seeded_multi_root_graphs_sweep()?;
    test_max_captured_descendants_truncation()?;
    test_max_captured_depth_truncation()?;
    test_real_os_capture()?;
    Ok(())
}

#[test]
fn capture_descendants_many_matches_per_root_over_frozen_table() -> Result<(), io::Error> {
    sim_capture_many_hierarchical_sweep()
}
