//! A macOS APFS volume on a RAM disk: the one home of the attach that `ci-local`'s
//! per-run scratch and `churn-slot`'s shared test volume both make.
//!
//! Included by path from both binaries, so the two cannot make a volume two ways.
//! A RAM disk takes memory only as it is written, which is why its size is a
//! ceiling the caller names and not a reservation.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

/// Attaches a RAM disk of `sectors` 512-byte sectors, formats it as APFS volume
/// `name` and returns the mounted volume and the device to detach it by.
pub(super) fn attach(name: &str, sectors: u64) -> std::io::Result<(PathBuf, String)> {
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
    let erased = Command::new("diskutil")
        .args(["erasevolume", "APFS", name, &device])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    let volume = Path::new("/Volumes").join(name);
    if erased.success() && volume.is_dir() {
        return Ok((volume, device));
    }
    run_quietly("hdiutil", &["detach", &device, "-force"]);
    lgwks_std::trace::debug!(?erased, device, "a ram disk could not be formatted");
    Err(std::io::Error::other(format!(
        "diskutil could not format {device} as {name} ({erased})"
    )))
}
