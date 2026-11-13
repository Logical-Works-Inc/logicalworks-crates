//! Reading a RustSec database and auditing lockfiles against it (#799, #942).
//!
//! The database is the layout `rustsec/advisory-db` keeps: `crates/<package>/`
//! and `rust/<component>/` directories of Markdown advisories. A dotfile, the
//! `RUSTSEC-0000-0000` template and a non-Markdown name are skipped; a name that
//! is not UTF-8 is refused, never skipped, because it can be neither matched nor
//! digested. Only pins whose source is a crates.io index are matched: a
//! workspace member, a path or git pin, or another registry is counted as
//! skipped, since no RustSec advisory names it.

use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::io::Read as _;
use std::path::{Path, PathBuf};

use super::parse::parse_advisory;
use super::semver::{Requirement, parse_requirement, parse_version};
use super::types::{
    Advisory, AdvisoryError, AdvisoryMatch, AdvisoryReport, DatabaseIdentity, Lockfile, err_bound,
    err_frontmatter, err_io, err_semver, refuse,
};
use crate::lock::Resolved;

/// The most lockfiles one audit reads.
pub(crate) const MAX_LOCKFILES: usize = 64;
/// The most pins one audit judges, over every lockfile.
pub(crate) const MAX_PACKAGES: usize = 65_536;
/// The most advisory files one database holds.
pub(crate) const MAX_ADVISORY_FILES: usize = 65_536;
/// The most bytes one advisory file holds: the largest in `advisory-db` is
/// under 20 KiB.
const MAX_ADVISORY_BYTES: usize = 1 << 20;
/// The most bytes one lockfile read from disk holds.
const MAX_LOCK_BYTES: usize = 64 << 20;
/// The source spellings of the crates.io index: the git index Cargo writes, and
/// the sparse index a source replacement can write.
const CRATES_IO: [&str; 2] = [
    "registry+https://github.com/rust-lang/crates.io-index",
    "sparse+https://index.crates.io/",
];

/// Computes a deterministic 128-bit FNV-1a digest over sorted relative paths and file contents.
#[must_use]
pub(crate) fn compute_db_digest<'a, I>(files: I) -> String
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    const OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    const PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
    let mut hash = OFFSET;
    let mut fold = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u128::from(*byte);
            hash = hash.wrapping_mul(PRIME);
        }
    };
    for (rel_path, content) in files {
        fold(rel_path.as_bytes());
        fold(b"\x1f");
        fold(content.as_bytes());
        fold(b"\n");
    }
    format!("fnv1a128:{hash:032x}")
}

/// Whether `source` is a crates.io index, the only source an advisory names.
#[must_use]
pub(crate) fn is_crates_io(source: Option<&str>) -> bool {
    source.is_some_and(|written| CRATES_IO.contains(&written))
}

/// What an advisory file name means: `Some` for an advisory to read, `None`
/// for a dotfile, the `RUSTSEC-0000-0000` template or a non-Markdown name.
///
/// # Errors
///
/// [`AdvisoryError::NotUtf8`] for a name that is not UTF-8.
pub(crate) fn advisory_name(name: &OsStr) -> Result<Option<&str>, AdvisoryError> {
    let Some(text) = name.to_str() else {
        return refuse(AdvisoryError::NotUtf8 {
            path: PathBuf::from(name),
        });
    };
    if text.starts_with('.') || text.starts_with("RUSTSEC-0000-0000") || !text.ends_with(".md") {
        return Ok(None);
    }
    Ok(Some(text))
}

/// The entries of `dir`, by name, sorted, each with whether it is a directory.
fn listing(dir: &Path, held: usize) -> Result<Vec<(OsString, bool)>, AdvisoryError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(cause) => return err_io(dir, cause),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(cause) => return err_io(dir, cause),
        };
        if held.saturating_add(found.len()) >= MAX_ADVISORY_FILES {
            return err_bound("advisory files in one database", MAX_ADVISORY_FILES);
        }
        let kind = match entry.file_type() {
            Ok(kind) => kind,
            Err(cause) => return err_io(&entry.path(), cause),
        };
        found.push((entry.file_name(), kind.is_dir()));
    }
    found.sort();
    Ok(found)
}

/// Every advisory file under `crates/*/` and `rust/*/`, as a relative path with
/// `/` separators and the full path, sorted.
fn advisory_files(db: &Path) -> Result<Vec<(String, PathBuf)>, AdvisoryError> {
    let mut files = Vec::new();
    for collection in ["crates", "rust"] {
        let root = db.join(collection);
        if !root.is_dir() {
            continue;
        }
        for (holder, is_dir) in listing(&root, files.len())? {
            if !is_dir {
                continue;
            }
            let Some(holder_name) = holder.to_str() else {
                return refuse(AdvisoryError::NotUtf8 {
                    path: root.join(&holder),
                });
            };
            if holder_name.starts_with('.') {
                continue;
            }
            let dir = root.join(&holder);
            for (name, inner_dir) in listing(&dir, files.len())? {
                let Ok(advisory) = advisory_name(&name) else {
                    return refuse(AdvisoryError::NotUtf8 {
                        path: dir.join(&name),
                    });
                };
                let Some(file) = advisory.filter(|_| !inner_dir) else {
                    continue;
                };
                files.push((format!("{collection}/{holder_name}/{file}"), dir.join(file)));
            }
        }
    }
    if files.is_empty() {
        return refuse(AdvisoryError::EmptyDatabase {
            path: db.to_path_buf(),
        });
    }
    files.sort();
    Ok(files)
}

/// The whole text of `path`, refused past `limit` bytes or when not UTF-8.
fn read_bounded(path: &Path, limit: usize, what: &'static str) -> Result<String, AdvisoryError> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(cause) => return err_io(path, cause),
    };
    let cap = match u64::try_from(limit) {
        Ok(bytes) => bytes.saturating_add(1),
        Err(_) => u64::MAX,
    };
    let mut text = String::new();
    if let Err(cause) = file.take(cap).read_to_string(&mut text) {
        return err_io(path, cause);
    }
    if text.len() > limit {
        return err_bound(what, limit);
    }
    Ok(text)
}

/// The database `HEAD` commit and committer date, when `db` is a git checkout.
fn database_head(db: &Path) -> Result<(Option<String>, Option<String>), AdvisoryError> {
    if !db.join(".git").exists() {
        return Ok((None, None));
    }
    let unreadable = |cause: String| {
        refuse(AdvisoryError::DatabaseHead {
            path: db.to_path_buf(),
            cause,
        })
    };
    let output = match crate::git::git(db, &["log", "-1", "--format=%H%n%cs", "HEAD"]) {
        Ok(output) => output,
        Err(cause) => return unreadable(format!("git could not be run: {cause}")),
    };
    if !output.status.success() {
        return unreadable(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = text.lines();
    let (Some(commit), Some(date)) = (lines.next(), lines.next()) else {
        return unreadable("git names no commit".to_owned());
    };
    let hex = commit.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !hex || !matches!(commit.len(), 40 | 64) || date.len() != 10 {
        return unreadable(format!("git answered {commit:?} {date:?}"));
    }
    Ok((Some(commit.to_owned()), Some(date.to_owned())))
}

/// One advisory with its requirements parsed once.
struct Compiled<'adv> {
    /// The advisory.
    advisory: &'adv Advisory,
    /// Its `patched` requirements.
    patched: Vec<Requirement>,
    /// Its `unaffected` requirements.
    unaffected: Vec<Requirement>,
}

/// Parses each requirement text.
fn requirements(texts: &[String]) -> Result<Vec<Requirement>, AdvisoryError> {
    texts.iter().map(|text| parse_requirement(text)).collect()
}

/// The live advisories by package name, each with its requirements parsed.
fn index(advisories: &[Advisory]) -> Result<HashMap<&str, Vec<Compiled<'_>>>, AdvisoryError> {
    let mut by_package: HashMap<&str, Vec<Compiled<'_>>> = HashMap::new();
    for advisory in advisories {
        if advisory.withdrawn().is_some() {
            continue;
        }
        let compiled = Compiled {
            advisory,
            patched: requirements(advisory.patched())?,
            unaffected: requirements(advisory.unaffected())?,
        };
        by_package
            .entry(advisory.package())
            .or_default()
            .push(compiled);
    }
    Ok(by_package)
}

/// Judges every pin of every lockfile: a crates.io pin matches each live
/// advisory for its name that no `patched` or `unaffected` requirement holds.
fn evaluate(
    lockfiles: &[(&str, Vec<Resolved>)],
    advisories: &[Advisory],
    database: DatabaseIdentity,
) -> Result<AdvisoryReport, AdvisoryError> {
    let by_package = index(advisories)?;
    let mut matches = Vec::new();
    let (mut audited, mut skipped) = (0_usize, 0_usize);
    for &(label, ref packages) in lockfiles {
        for pkg in packages {
            if audited.saturating_add(skipped) >= MAX_PACKAGES {
                return err_bound("pinned packages in one audit", MAX_PACKAGES);
            }
            if !is_crates_io(pkg.source()) {
                skipped = skipped.saturating_add(1);
                continue;
            }
            audited = audited.saturating_add(1);
            let Ok(version) = parse_version(pkg.version()) else {
                return err_semver(pkg.version());
            };
            let Some(candidates) = by_package.get(pkg.name()) else {
                continue;
            };
            for compiled in candidates {
                let excluded = compiled
                    .patched
                    .iter()
                    .chain(&compiled.unaffected)
                    .any(|requirement| requirement.matches(&version));
                if !excluded {
                    matches.push(AdvisoryMatch::new(compiled.advisory, pkg.version(), label));
                }
            }
        }
    }
    matches.sort_by(|lhs, rhs| {
        (
            lhs.advisory_id(),
            lhs.package(),
            lhs.installed_version(),
            lhs.lockfile(),
        )
            .cmp(&(
                rhs.advisory_id(),
                rhs.package(),
                rhs.installed_version(),
                rhs.lockfile(),
            ))
    });
    matches.dedup();
    Ok(AdvisoryReport {
        database,
        lockfiles: lockfiles.len(),
        packages_audited: audited,
        packages_skipped: skipped,
        matches,
    })
}

/// Evaluates one lockfile's resolved packages against in-memory advisories.
#[cfg(test)]
pub(crate) fn evaluate_lock(
    lock_packages: &[Resolved],
    advisories: &[Advisory],
    db_digest: String,
) -> Result<AdvisoryReport, AdvisoryError> {
    let database = DatabaseIdentity {
        digest: db_digest,
        commit: None,
        date: None,
        advisories: advisories.len(),
    };
    evaluate(
        &[("Cargo.lock", lock_packages.to_vec())],
        advisories,
        database,
    )
}

/// Loads every advisory file under `db`, checking each requirement parses and
/// has a shape RustSec loads, and names the database.
fn load(db: &Path) -> Result<(Vec<Advisory>, DatabaseIdentity), AdvisoryError> {
    if !db.is_dir() {
        return err_io(db, "advisory database directory does not exist");
    }
    let (commit, date) = database_head(db)?;
    let mut read = Vec::new();
    for (rel, path) in advisory_files(db)? {
        let text = read_bounded(&path, MAX_ADVISORY_BYTES, "bytes in one advisory file")?;
        read.push((rel, path, text));
    }
    let digest = compute_db_digest(
        read.iter()
            .map(|entry| (entry.0.as_str(), entry.2.as_str())),
    );
    let mut advisories = Vec::with_capacity(read.len());
    for entry in &read {
        let path = &entry.1;
        let advisory = parse_advisory(path, &entry.2)?;
        for written in advisory.patched().iter().chain(advisory.unaffected()) {
            let Ok(requirement) = parse_requirement(written) else {
                return err_frontmatter(path, format!("{written:?} is not a version requirement"));
            };
            if let Err(shape) = requirement.rustsec_shape() {
                return refuse(AdvisoryError::UnsupportedRequirement {
                    path: path.clone(),
                    text: written.clone(),
                    reason: shape.reason(),
                });
            }
        }
        advisories.push(advisory);
    }
    let database = DatabaseIdentity {
        digest,
        commit,
        date,
        advisories: advisories.len(),
    };
    Ok((advisories, database))
}

/// Audits `lockfiles` against the RustSec database checked out at `database`.
///
/// Every crates.io pin is judged as Cargo's `VersionReq` reads the advisory's
/// requirements, pre-release gate included, so a pre-release is never patched
/// by a plain bound. Vulnerabilities and informational notices are both
/// returned; an `[affected]` os or arch restriction is carried on the match,
/// not applied. `tests/it/sim_advisories_audit.rs` drives it end to end.
///
/// # Errors
///
/// [`AdvisoryError`] when a lockfile or the database cannot be read, an
/// advisory or a version in them is malformed, or an input passes a bound: a
/// refusal, never a clean report.
#[doc(hidden)]
pub fn audit(lockfiles: &[Lockfile<'_>], database: &Path) -> Result<AdvisoryReport, AdvisoryError> {
    if lockfiles.len() > MAX_LOCKFILES {
        return err_bound("lockfiles in one audit", MAX_LOCKFILES);
    }
    let mut parsed = Vec::with_capacity(lockfiles.len());
    for lockfile in lockfiles {
        match crate::lock::parse(lockfile.text()) {
            Ok(packages) => parsed.push((lockfile.label(), packages)),
            Err(cause) => return refuse(AdvisoryError::Lock(cause)),
        }
    }
    let (advisories, identity) = load(database)?;
    evaluate(&parsed, &advisories, identity)
}

/// Audits the `Cargo.lock` files at `lock_paths` against the database at
/// `db_path`, naming each lockfile by the path given. `lgwks-deps advisories`
/// is this call.
///
/// # Errors
///
/// As [`audit`], and when a lockfile cannot be read or passes 64 MiB.
#[doc(hidden)]
pub fn audit_files(
    lock_paths: &[PathBuf],
    db_path: &Path,
) -> Result<AdvisoryReport, AdvisoryError> {
    if lock_paths.len() > MAX_LOCKFILES {
        return err_bound("lockfiles in one audit", MAX_LOCKFILES);
    }
    let mut read = Vec::with_capacity(lock_paths.len());
    for lock_path in lock_paths {
        let text = read_bounded(lock_path, MAX_LOCK_BYTES, "bytes in one lockfile")?;
        read.push((lock_path.to_string_lossy(), text));
    }
    let lockfiles: Vec<Lockfile<'_>> = read
        .iter()
        .map(|entry| Lockfile::new(&entry.0, &entry.1))
        .collect();
    audit(&lockfiles, db_path)
}
