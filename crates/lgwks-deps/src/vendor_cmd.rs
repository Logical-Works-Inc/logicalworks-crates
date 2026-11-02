//! The `vendor check` command: proves every locked package resolves to the
//! shared vendor tree, so an offline build never reaches the network.

use std::io;
use std::path::Path;
use std::process::ExitCode;

use crate::{USAGE, read_lock_text, refuse, resolve_repository_root, subject_path};

/// Handles `vendor check`, rejecting any other `vendor` subcommand.
pub(crate) fn handle_vendor(
    args: &[String],
    out: &mut impl io::Write,
    err: &mut impl io::Write,
) -> io::Result<ExitCode> {
    if args.first().map(String::as_str) != Some("check") {
        write!(err, "{USAGE}")?;
        return Ok(ExitCode::from(2));
    }
    let positional: Vec<&String> = args[1..]
        .iter()
        .filter(|arg| !arg.starts_with("--"))
        .collect();
    let start = subject_path(
        positional
            .first()
            .map(|candidate| Path::new(candidate.as_str())),
    );
    run_vendor_check(&start, out, err)
}

/// Writes the refusal for a lock file the vendor tree does not cover: how many
/// locked packages are missing, each one by name and version, and the repair.
fn write_missing_packages(
    root: &Path,
    tree: &Path,
    report: &lgwks_deps::vendor::Report,
    err: &mut impl io::Write,
) -> io::Result<()> {
    writeln!(
        err,
        "REFUSED  {} — {} of {} locked packages missing from {}\n",
        root.display(),
        report.missing().len(),
        // Both operands are counts of entries in one lock file, so the
        // sum is bounded by its package count and cannot overflow.
        report.covered().saturating_add(report.missing().len()),
        tree.display()
    )?;
    for missing in report.missing() {
        writeln!(err, "  {} {}", missing.name(), missing.version())?;
    }
    writeln!(
        err,
        "\nRe-run the vendor sync for this repo, then re-check."
    )
}

/// Proves every locked package resolves to the shared vendor tree.
///
/// Exit 0 when the tree covers the lock file and 2 when any locked package is
/// missing from it, since an uncovered package means the offline build would
/// reach the network.
fn run_vendor_check(
    start: &Path,
    out: &mut impl io::Write,
    err: &mut impl io::Write,
) -> io::Result<ExitCode> {
    let root = unwrap_or_refuse!(resolve_repository_root(start), err);
    let tree = match lgwks_deps::vendor::tree_for(&root) {
        Ok(tree) => tree,
        Err(error) => return refuse(&error.to_string(), err),
    };
    let lock_text = unwrap_or_refuse!(read_lock_text(&root), err);
    match lgwks_deps::vendor::check_coverage(&lock_text, &tree) {
        Ok(report) if report.is_complete() => {
            writeln!(
                out,
                "OK  {} — {} locked packages covered by {}, {} local skipped",
                root.display(),
                report.covered(),
                tree.display(),
                report.skipped_local()
            )?;
            Ok(ExitCode::SUCCESS)
        }
        Ok(report) => {
            write_missing_packages(&root, &tree, &report, err)?;
            Ok(ExitCode::from(2))
        }
        Err(error) => refuse(&error.to_string(), err),
    }
}
