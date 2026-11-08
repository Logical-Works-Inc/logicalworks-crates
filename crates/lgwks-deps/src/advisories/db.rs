//! Database digest computation and audit evaluation for RustSec (#799).

use std::path::{Path, PathBuf};

use super::parse::parse_advisory;
use super::semver::{parse_requirement, parse_version};
use super::types::{
    Advisory, AdvisoryError, AdvisoryMatch, AdvisoryReport, err_io, err_lock, err_semver,
};
use crate::lock::Resolved;

/// Computes a deterministic 128-bit FNV-1a digest over sorted relative paths and file contents.
#[must_use]
pub(crate) fn compute_db_digest<'a, I>(files: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;

    for (rel_path, content) in files {
        for byte in rel_path.as_bytes() {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        hash ^= u128::from(b'\x1f');
        hash = hash.wrapping_mul(PRIME);
        for byte in content.as_bytes() {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
        hash ^= u128::from(b'\n');
        hash = hash.wrapping_mul(PRIME);
    }

    format!("fnv1a128:{hash:032x}")
}

/// Evaluates resolved lockfile packages against an advisory database.
pub(crate) fn evaluate_lock(
    lock_packages: &[Resolved],
    advisories: &[Advisory],
    db_digest: String,
) -> Result<AdvisoryReport, AdvisoryError> {
    let mut matches = Vec::new();
    let mut packages_audited: usize = 0;

    for pkg in lock_packages {
        if pkg.is_local() {
            continue;
        }
        packages_audited = packages_audited.saturating_add(1);

        let version = match parse_version(pkg.version()) {
            Ok(parsed_ver) => parsed_ver,
            Err(_) => return err_semver(pkg.version()),
        };

        for adv in advisories {
            if adv.withdrawn().is_some() {
                continue;
            }
            if adv.package() != pkg.name() {
                continue;
            }

            let mut is_patched = false;
            for req_str in adv.patched() {
                let req = match parse_requirement(req_str) {
                    Ok(parsed_req) => parsed_req,
                    Err(_) => return err_semver(req_str),
                };
                if req.matches(&version) {
                    is_patched = true;
                    break;
                }
            }
            if is_patched {
                continue;
            }

            let mut is_unaffected = false;
            for req_str in adv.unaffected() {
                let req = match parse_requirement(req_str) {
                    Ok(parsed_req) => parsed_req,
                    Err(_) => return err_semver(req_str),
                };
                if req.matches(&version) {
                    is_unaffected = true;
                    break;
                }
            }
            if is_unaffected {
                continue;
            }

            matches.push(AdvisoryMatch::new(
                adv.id(),
                pkg.name(),
                pkg.version(),
                adv.kind().clone(),
                adv.patched().to_vec(),
            ));
        }
    }

    matches.sort_by(|lhs, rhs| {
        lhs.advisory_id()
            .cmp(rhs.advisory_id())
            .then_with(|| lhs.package().cmp(rhs.package()))
            .then_with(|| lhs.installed_version().cmp(rhs.installed_version()))
    });
    matches.dedup();

    Ok(AdvisoryReport::new(
        db_digest,
        advisories.len(),
        packages_audited,
        matches,
    ))
}

/// Recursively discovers and loads all `RUSTSEC-*.md` advisory files under `db_path`.
fn load_advisories_and_digest(db_path: &Path) -> Result<(Vec<Advisory>, String), AdvisoryError> {
    if !db_path.exists() {
        return err_io(db_path, "advisory database directory does not exist");
    }

    let mut dirs_to_visit = vec![db_path.to_path_buf()];
    let mut collected_files: Vec<(String, PathBuf, String)> = Vec::new();

    while let Some(current_dir) = dirs_to_visit.pop() {
        let entries = match std::fs::read_dir(&current_dir) {
            Ok(entries_iter) => entries_iter,
            Err(cause) => return err_io(&current_dir, cause),
        };

        for entry_res in entries {
            let entry = match entry_res {
                Ok(dir_entry) => dir_entry,
                Err(cause) => return err_io(&current_dir, cause),
            };

            let entry_path = entry.path();
            let file_type = match entry.file_type() {
                Ok(ft_val) => ft_val,
                Err(cause) => return err_io(&entry_path, cause),
            };

            let file_name = entry.file_name();
            let name_lossy = file_name.to_string_lossy();

            if file_type.is_dir() {
                if !name_lossy.starts_with('.') {
                    dirs_to_visit.push(entry_path);
                }
            } else if file_type.is_file()
                && name_lossy.starts_with("RUSTSEC-")
                && name_lossy.ends_with(".md")
            {
                let content = match std::fs::read_to_string(&entry_path) {
                    Ok(content_str) => content_str,
                    Err(cause) => return err_io(&entry_path, cause),
                };

                let rel = match entry_path.strip_prefix(db_path) {
                    Ok(rel_path) => rel_path,
                    Err(_) => &entry_path,
                };
                let rel_str = rel.to_string_lossy().replace('\\', "/");
                collected_files.push((rel_str, entry_path, content));
            }
        }
    }

    collected_files.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));

    let digest_input: Vec<(&str, &str)> = collected_files
        .iter()
        .map(|entry| (entry.0.as_str(), entry.2.as_str()))
        .collect();
    let db_digest = compute_db_digest(digest_input);

    let mut advisories = Vec::with_capacity(collected_files.len());
    for (_, path, content) in collected_files {
        let advisory = parse_advisory(&path, &content)?;
        advisories.push(advisory);
    }

    Ok((advisories, db_digest))
}

/// Audits a `Cargo.lock` file against a local RustSec advisory database directory.
///
/// # Errors
///
/// [`AdvisoryError`] when the lockfile or the database cannot be read, or an
/// advisory or a version in them is malformed: a refusal, never a clean report.
pub(crate) fn audit_lockfile(
    lock_path: &Path,
    db_path: &Path,
) -> Result<AdvisoryReport, AdvisoryError> {
    let lock_text = match std::fs::read_to_string(lock_path) {
        Ok(text) => text,
        Err(cause) => return err_io(lock_path, cause),
    };

    let lock_packages = match crate::lock::parse(&lock_text) {
        Ok(packages) => packages,
        Err(cause) => return err_lock(cause),
    };

    let (advisories, db_digest) = load_advisories_and_digest(db_path)?;
    evaluate_lock(&lock_packages, &advisories, db_digest)
}
