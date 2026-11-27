//! A macOS APFS volume on a RAM disk: the one home of the attach that `ci-local`'s
//! per-run scratch and `churn-slot`'s shared test volume both make.
//!
//! Included by path from both binaries, so the two cannot make a volume two ways.
//! A RAM disk takes memory only as it is written, which is why its size is a
//! ceiling the caller names and not a reservation.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// A device's hold on this process's lifeline (ADR 0087).
#[cfg(unix)]
pub(super) type Hold = lgwks_std::process::LifelineHold;

/// No lifeline off Unix, and no RAM disk to hold: `attach` refuses there.
#[cfg(not(unix))]
#[derive(Debug)]
pub(super) struct Hold;

/// Hold `device` on the lifeline.
#[cfg(unix)]
fn hold_device(device: &str) -> std::io::Result<Hold> {
    lgwks_std::process::hold_device(device)
}

/// Off Unix there is no lifeline, so a device found there is refused rather
/// than left unowned.
#[cfg(not(unix))]
fn hold_device(device: &str) -> std::io::Result<Hold> {
    lgwks_std::trace::debug!(device, "no lifeline off unix; the ram disk is refused");
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "a RAM disk is held on a lifeline, which only Unix has",
    ))
}

/// Runs a program for its effect, logging a failure.
pub(super) fn run_quietly(program: &str, args: &[&str]) {
    let outcome = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match outcome {
        Ok(status) if status.success() => {}
        Ok(status) => {
            lgwks_std::trace::debug!(program, ?status, "a ram disk command did not succeed")
        }
        Err(error) => lgwks_std::trace::debug!(%error, program, "a ram disk command could not run"),
    }
}

/// Turns fseventsd and Spotlight off for `volume`. fseventsd logs every write
/// on every mounted volume and keeps what it logs in memory until it flushes;
/// the gate's per-run RAM disks took it to 3.9 GB RSS on the gate Mac
/// (2026-11-27, #1160). Apple's FSEvents Programming Guide, "Preventing File
/// System Event Storage": an empty `.fseventsd/no_log` at the volume root
/// stops it. fseventsd reads the file when the volume mounts, so the volume
/// is mounted again after the file is written: without that it logged 20,000
/// writes all the same (`no_log_volume_logs_no_event_after_a_write_burst`).
fn quiet_events(volume: &Path) -> std::io::Result<()> {
    let events = volume.join(".fseventsd");
    std::fs::create_dir_all(&events)?;
    std::fs::File::create(events.join("no_log"))?;
    std::fs::File::create(volume.join(".metadata_never_index"))?;
    remount(volume)
}

/// Unmounts `volume` and mounts its device again at the same place.
pub(super) fn remount(volume: &Path) -> std::io::Result<()> {
    let shown = volume.display().to_string();
    let info = Command::new("diskutil")
        .args(["info", &shown])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    let text = String::from_utf8_lossy(&info.stdout).into_owned();
    let Some(id) = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("Device Identifier:"))
        .map(str::trim)
    else {
        lgwks_std::trace::debug!(volume = shown, "diskutil named no device for a volume");
        return Err(std::io::Error::other(format!(
            "diskutil named no device for {shown}"
        )));
    };
    for args in [["unmount", shown.as_str()], ["mount", id]] {
        let status = Command::new("diskutil")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            lgwks_std::trace::debug!(?status, ?args, "a ram disk remount step failed");
            return Err(std::io::Error::other(format!(
                "diskutil {args:?} failed: {status}"
            )));
        }
    }
    Ok(())
}

/// Attaches a RAM disk of `sectors` 512-byte sectors, formats it as APFS volume
/// `name` and returns the mounted volume, the device to detach it by, and the
/// device's hold on this process's lifeline.
///
/// The device is held the moment `hdiutil` names it, before the format: a run
/// SIGKILLed between the two once left an unmounted 4 GiB RAM disk no name
/// could find (#1160). While the hold is open, this process's death by any
/// path detaches the device; a caller that keeps the volume past its own life
/// drops the hold.
pub(super) fn attach(name: &str, sectors: u64) -> std::io::Result<(PathBuf, String, Hold)> {
    let attached = Command::new("hdiutil")
        .args(["attach", "-nomount", &format!("ram://{sectors}")])
        .stdin(Stdio::null())
        .output()?;
    let text = String::from_utf8_lossy(&attached.stdout).into_owned();
    let Some(device) = text
        .split_whitespace()
        .find(|word| word.starts_with("/dev/disk"))
        .map(str::to_owned)
    else {
        let said = String::from_utf8_lossy(&attached.stderr).into_owned();
        lgwks_std::trace::debug!(said, "hdiutil attached no ram disk");
        return Err(std::io::Error::other(format!(
            "hdiutil attached no ram disk: {}",
            said.trim()
        )));
    };
    let hold = match hold_device(&device) {
        Ok(hold) => hold,
        Err(error) => {
            run_quietly("hdiutil", &["detach", &device, "-force"]);
            lgwks_std::trace::debug!(%error, device, "a ram disk could not be held, so it was detached");
            return Err(error);
        }
    };
    let erased = Command::new("diskutil")
        .args(["erasevolume", "APFS", name, &device])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let volume = Path::new("/Volumes").join(name);
    if erased.success() && volume.is_dir() {
        match quiet_events(&volume) {
            Ok(()) => return Ok((volume, device, hold)),
            Err(error) => {
                lgwks_std::trace::debug!(%error, device, "fseventsd could not be turned off for a ram disk, so it is let go");
                let _released_after_detach = hold;
                run_quietly("hdiutil", &["detach", &device, "-force"]);
                return Err(error);
            }
        }
    }
    {
        // Let go once the device is detached. Bound, not passed to `drop`: off
        // Unix a `Hold` has no destructor, and Windows clippy refuses `drop` of
        // one (`drop_non_drop`, main run 45917420774).
        let _released_after_detach = hold;
        run_quietly("hdiutil", &["detach", &device, "-force"]);
    }
    lgwks_std::trace::debug!(?erased, device, "a ram disk could not be formatted");
    Err(std::io::Error::other(format!(
        "diskutil could not format {device} as {name} ({erased})"
    )))
}
