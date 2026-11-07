//! Batch descendant capture across multiple root processes in one snapshot (#798).

use std::collections::{BTreeMap, BTreeSet};
use std::io;

#[cfg(all(any(unix, windows), feature = "process"))]
use super::capture::{ContainmentMechanism, DescendantSet, MAX_CAPTURED_DESCENDANTS};
#[cfg(all(any(unix, windows), feature = "process"))]
use super::invalid_pid;
#[cfg(all(not(any(unix, windows)), feature = "process"))]
use super::unsupported;

/// The most root process ids one batch descendant capture accepts.
///
/// Refuses unbounded caller slices before reading any process table, protecting
/// against memory exhaustion or denial of service when querying many roots. Past
/// this bound, [`capture_descendants_many`] returns [`std::io::ErrorKind::InvalidInput`].
#[doc(hidden)]
pub const MAX_CAPTURE_ROOTS: usize = 16_384;

#[cfg(all(any(unix, windows), feature = "process"))]
fn validate_common_roots(roots: &[i32]) -> io::Result<()> {
    if roots.len() > MAX_CAPTURE_ROOTS {
        let refusal = Err(invalid_pid());
        #[cfg(feature = "trace")]
        crate::trace::debug!(
            error = ?refusal.as_ref().err(),
            roots_len = roots.len(),
            max = MAX_CAPTURE_ROOTS,
            "capture_descendants_many: roots length exceeds MAX_CAPTURE_ROOTS"
        );
        return refusal;
    }
    for root in roots {
        if *root <= 0 {
            let refusal = Err(invalid_pid());
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                error = ?refusal.as_ref().err(),
                root = *root,
                "capture_descendants_many: non-positive root process id"
            );
            return refusal;
        }
    }
    Ok(())
}

#[cfg(all(target_os = "macos", feature = "process"))]
fn read_table_pairs() -> io::Result<Vec<(i32, i32)>> {
    Ok(super::darwin::table()?
        .into_iter()
        .map(|row| (row.pid, row.ppid))
        .collect())
}

#[cfg(all(target_os = "linux", feature = "process"))]
fn read_table_pairs() -> io::Result<Vec<(i32, i32)>> {
    let mut pairs = Vec::new();
    let entries = std::fs::read_dir("/proc")?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<i32>() else {
            continue;
        };
        if pid <= 0 {
            continue;
        }
        let stat_path = entry.path().join("stat");
        if let Ok(stat) = std::fs::read_to_string(stat_path) {
            if let Some(ppid) = parse_stat_ppid(&stat) {
                pairs.push((pid, ppid));
            }
        }
    }
    Ok(pairs)
}

#[cfg(all(target_os = "linux", feature = "process"))]
fn parse_stat_ppid(stat: &str) -> Option<i32> {
    let start_idx = stat.rfind(')')?.saturating_add(1);
    let rest = stat.get(start_idx..)?;
    let mut parts = rest.split_ascii_whitespace();
    let _state = parts.next()?;
    let ppid_str = parts.next()?;
    ppid_str.parse::<i32>().ok()
}

#[cfg(all(windows, feature = "process"))]
fn read_table_pairs() -> io::Result<Vec<(i32, i32)>> {
    Ok(super::windows::snapshot(&super::windows::NATIVE)?
        .into_iter()
        .filter_map(|(pid, parent)| Some((i32::try_from(pid).ok()?, i32::try_from(parent).ok()?)))
        .collect())
}

/// Linux and macOS: reads a single process table snapshot and traverses each
/// root in `roots`. Bounded by [`MAX_CAPTURE_ROOTS`].
///
/// # Errors
///
/// Returns [`std::io::ErrorKind::InvalidInput`] if `roots.len() > MAX_CAPTURE_ROOTS`
/// or if any root in `roots` is non-positive (`<= 0`). Returns an error if the
/// process table cannot be read.
#[doc(hidden)]
#[cfg(all(unix, feature = "process"))]
pub fn capture_descendants_many(roots: &[i32]) -> io::Result<BTreeMap<i32, DescendantSet>> {
    validate_common_roots(roots)?;
    if roots.is_empty() {
        return Ok(BTreeMap::new());
    }

    #[cfg(target_os = "linux")]
    if roots.len() <= 32 {
        let mut proc_sets = BTreeMap::new();
        let mut all_succeeded = true;
        for root in roots {
            let root_pid = *root;
            if let Ok(set) = super::capture_descendants(root_pid) {
                proc_sets.insert(root_pid, set);
            } else {
                all_succeeded = false;
                break;
            }
        }
        if all_succeeded {
            return Ok(proc_sets);
        }
    }

    load_pairs_and_capture(roots)
}

/// Windows: reads a single Toolhelp snapshot and traverses each root in
/// `roots`. Bounded by [`MAX_CAPTURE_ROOTS`]. Validates each root via
/// `super::windows::pid_u32`.
///
/// # Errors
///
/// Returns [`std::io::ErrorKind::InvalidInput`] if `roots.len() > MAX_CAPTURE_ROOTS`
/// or if any root in `roots` is non-positive (`<= 0`) or an invalid Windows pid.
#[doc(hidden)]
#[cfg(all(windows, feature = "process"))]
pub fn capture_descendants_many(roots: &[i32]) -> io::Result<BTreeMap<i32, DescendantSet>> {
    validate_common_roots(roots)?;
    for root in roots {
        if let Err(error) = super::windows::pid_u32(*root) {
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                error = ?error,
                root = *root,
                "capture_descendants_many: invalid windows pid"
            );
            return Err(error);
        }
    }
    if roots.is_empty() {
        return Ok(BTreeMap::new());
    }

    load_pairs_and_capture(roots)
}

/// Every process descended from each root in `roots` at the moment of the call.
///
/// The `process` capability is Unix and Windows; other targets report
/// [`std::io::ErrorKind::Unsupported`] rather than a fabricated empty tree.
#[doc(hidden)]
#[cfg(all(not(any(unix, windows)), feature = "process"))]
pub fn capture_descendants_many(roots: &[i32]) -> io::Result<BTreeMap<i32, DescendantSet>> {
    let _ = roots;
    let refusal = Err(unsupported("process capture_descendants_many is Unix-only"));
    #[cfg(feature = "trace")]
    crate::trace::debug!(
        error = ?refusal.as_ref().err(),
        "capture_descendants_many: returning an error to the caller"
    );
    refusal
}

#[cfg(all(any(unix, windows), feature = "process"))]
fn load_pairs_and_capture(roots: &[i32]) -> io::Result<BTreeMap<i32, DescendantSet>> {
    let pairs = match read_table_pairs() {
        Ok(read_pairs) => read_pairs,
        Err(error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(
                error = ?error,
                "capture_descendants_many: read_table_pairs failed"
            );
            return Err(error);
        }
    };
    capture_many_from_pairs(roots, &pairs)
}

/// Capture descendants for multiple roots from pre-computed `(pid, ppid)` pairs.
///
/// Bounded by [`MAX_CAPTURE_ROOTS`] on the root slice length and validates that
/// every root is a positive process id. Traverses each root independently using
/// a breadth-first search over `pairs` so that overlapping trees (e.g. where one
/// root is a descendant of another) each receive their complete set of descendants.
///
/// # Errors
///
/// Returns [`std::io::ErrorKind::InvalidInput`] if `roots.len() > MAX_CAPTURE_ROOTS`
/// or if any root in `roots` is non-positive (`<= 0`).
#[cfg(all(any(unix, windows), feature = "process"))]
pub(super) fn capture_many_from_pairs(
    roots: &[i32],
    pairs: &[(i32, i32)],
) -> io::Result<BTreeMap<i32, DescendantSet>> {
    validate_common_roots(roots)?;
    if roots.is_empty() {
        return Ok(BTreeMap::new());
    }

    let mut children_of: BTreeMap<i32, Vec<i32>> = BTreeMap::new();
    for pair in pairs {
        let (pid, parent) = *pair;
        if pid <= 0 {
            continue;
        }
        children_of.entry(parent).or_default().push(pid);
    }

    let mut result = BTreeMap::new();
    for root in roots {
        let root_pid = *root;
        let mut pids = Vec::new();
        let mut truncated = false;
        let mut frontier = vec![root_pid];
        let mut visited = BTreeSet::from([root_pid]);

        let mut completed_whole_tree = false;
        for _depth in 0..64 {
            let mut next = Vec::new();
            for parent in frontier {
                if let Some(children) = children_of.get(&parent) {
                    for child in children {
                        let child_pid = *child;
                        if child_pid <= 0 || !visited.insert(child_pid) {
                            continue;
                        }
                        if pids.len() >= MAX_CAPTURED_DESCENDANTS {
                            truncated = true;
                            break;
                        }
                        pids.push(child_pid);
                        next.push(child_pid);
                    }
                }
                if truncated {
                    break;
                }
            }
            if truncated {
                break;
            }
            if next.is_empty() {
                completed_whole_tree = true;
                break;
            }
            frontier = next;
        }

        if !completed_whole_tree {
            truncated = true;
        }

        pids.sort_unstable();
        pids.dedup();
        let set = DescendantSet::from_raw_parts(
            ContainmentMechanism::ProcessTableSnapshot,
            pids,
            truncated,
        );
        result.insert(root_pid, set);
    }
    Ok(result)
}
