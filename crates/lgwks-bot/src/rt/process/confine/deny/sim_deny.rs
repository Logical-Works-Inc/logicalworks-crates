//! The denial wrappers against the targets they confine. `user_namespaces(7)`:
//! a process that makes a user namespace holds every capability inside it, so
//! the fallback's mapped root could `umount` its own overmount unless the exec
//! drops them. #1136: a `--bind` root is `nodev`, and the systemd user manager
//! starts processes outside the namespace.

use std::io;
use std::path::Path;

use super::{BWRAP, UNSHARE, bwrap_deny, denial_wrapper, unshare_deny};

/// What the payload's secret reads as when the denial held.
const SECRET: &str = "the-hidden-bytes";

/// How the denial failed, or how the attempt to judge it did.
enum DenyFault {
    /// The scratch directory or the wrapper would not run.
    Setup(io::Error),
    /// The host had no entropy to name the scratch directory with.
    Entropy(lgwks_std::random::EntropyError),
    /// The payload read the secret through the overmount.
    Lifted,
    /// The payload still held capabilities after the wrapper's exec.
    KeptCapabilities,
    /// The payload could not write `/dev/null` under the denial.
    LostDevices,
    /// The user manager read the secret for the payload.
    ManagerRead,
}

/// Written by hand because a test's returned error is printed through `Debug`,
/// and a derived `Debug` does not count as reading a cause, which `dead_code`
/// then refuses on the Linux lanes.
impl core::fmt::Debug for DenyFault {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Setup(ref cause) => write!(f, "setup failed: {cause}"),
            Self::Entropy(ref cause) => write!(f, "no entropy: {cause}"),
            Self::Lifted => f.write_str("the payload read the secret through the overmount"),
            Self::KeptCapabilities => f.write_str("the payload kept capabilities after exec"),
            Self::LostDevices => f.write_str("the payload could not write /dev/null"),
            Self::ManagerRead => f.write_str("the user manager read the secret for the payload"),
        }
    }
}

impl From<io::Error> for DenyFault {
    fn from(error: io::Error) -> Self {
        Self::Setup(error)
    }
}

impl From<lgwks_std::random::EntropyError> for DenyFault {
    fn from(error: lgwks_std::random::EntropyError) -> Self {
        Self::Entropy(error)
    }
}

/// Catches the escape this wrapper shipped with: a target of `unshare -m -r`
/// that ran `umount` on the denied path and read what the tmpfs hid.
///
/// A host that refuses the namespace cannot reach the payload at all. That is
/// the cap, named on the trace; hosted Ubuntu 24.04 restricts unprivileged user
/// namespaces this way, and the wrapper fails closed there.
#[test]
fn sim_deny_saboteur_the_fallback_payload_cannot_lift_the_overmount() -> Result<(), DenyFault> {
    if !Path::new(UNSHARE).exists() {
        lgwks_std::trace::warn!("cap: this host has no unshare");
        return Ok(());
    }
    let held = std::env::temp_dir().join(format!("lgwks-deny-{}", lgwks_std::id::Uuid::new_v4()?));
    std::fs::create_dir_all(&held)?;
    let outcome = attack(&held);
    std::fs::remove_dir_all(&held)?;
    outcome
}

/// Runs the payload against a denial of `held` and judges what it saw.
fn attack(held: &Path) -> Result<(), DenyFault> {
    std::fs::write(held.join("secret"), SECRET)?;
    let mut command = unshare_deny(std::iter::once(held));
    command
        .arg("/bin/sh")
        .arg("-c")
        .arg(r#"umount "$HELD" 2>/dev/null; cat "$HELD/secret" 2>/dev/null; while read -r key value; do [ "$key" = "CapEff:" ] && echo "caps=$value"; done < /proc/self/status"#)
        .env("HELD", held);
    let output = command.output()?;
    let printed = String::from_utf8_lossy(&output.stdout);
    if !printed.contains("caps=") {
        lgwks_std::trace::warn!(
            status = ?output.status.code(),
            "cap: this host refused the namespace before the payload ran"
        );
        return Ok(());
    }
    let verdict = if printed.contains(SECRET) {
        Err(DenyFault::Lifted)
    } else if printed.contains("caps=0000000000000000") {
        Ok(())
    } else {
        Err(DenyFault::KeptCapabilities)
    };
    lgwks_std::trace::debug!(?verdict, %printed, "sim_deny: the payload's report");
    verdict
}

/// A fresh scratch directory under the temp dir, named from entropy.
fn scratch(label: &str) -> Result<std::path::PathBuf, DenyFault> {
    let dir =
        std::env::temp_dir().join(format!("lgwks-{label}-{}", lgwks_std::id::Uuid::new_v4()?));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// #1136 item 1: `bwrap --bind / /` mounts the root `nodev`, so no build tool
/// could write `/dev/null`. The denial binds it with devices.
#[test]
fn sim_deny_a_bwrap_denial_keeps_the_devices() -> Result<(), DenyFault> {
    if !Path::new(BWRAP).exists() {
        lgwks_std::trace::warn!("cap: this host has no bwrap");
        return Ok(());
    }
    let held = scratch("deny-dev")?;
    let mut command = bwrap_deny(std::iter::once(held.as_path()));
    command.args([
        "/bin/sh",
        "-c",
        "echo started; : >/dev/null && echo devices",
    ]);
    let output = command.output();
    std::fs::remove_dir_all(&held)?;
    let printed = String::from_utf8_lossy(&output?.stdout).into_owned();
    if !printed.contains("started") {
        lgwks_std::trace::warn!("cap: this host refused the namespace before the payload ran");
        return Ok(());
    }
    if printed.contains("devices") {
        Ok(())
    } else {
        Err(DenyFault::LostDevices)
    }
}

/// #1136 item 2: a process the systemd user manager starts is not the child's
/// descendant, so it reads the host's tree. The denial hides the manager's
/// runtime directory, so a confined `systemd-run --user` cannot reach it. A
/// control run outside the denial proves the manager reads the file on this
/// host; a host with no user manager is the cap.
#[test]
fn sim_deny_saboteur_the_user_manager_cannot_read_a_denied_file() -> Result<(), DenyFault> {
    const SYSTEMD_RUN: &str = "/usr/bin/systemd-run";
    const TIMEOUT: &str = "/usr/bin/timeout";
    if !Path::new(SYSTEMD_RUN).exists() || !Path::new(TIMEOUT).exists() {
        lgwks_std::trace::warn!("cap: this host has no systemd-run or timeout");
        return Ok(());
    }
    let held = scratch("deny-manager")?;
    let secret = held.join("secret");
    std::fs::write(&secret, SECRET)?;
    let payload = |command: &mut std::process::Command| -> io::Result<String> {
        command
            .args([
                TIMEOUT,
                "20",
                SYSTEMD_RUN,
                "--user",
                "--wait",
                "--pipe",
                "-q",
                "/bin/cat",
            ])
            .arg(&secret)
            .stdin(std::process::Stdio::null());
        Ok(String::from_utf8_lossy(&command.output()?.stdout).into_owned())
    };
    let control = payload(&mut std::process::Command::new("/usr/bin/env"));
    let mut denied = super::DenyBeneath::new();
    denied.deny(&held);
    let confined = denial_wrapper(&denied).and_then(|mut wrapper| payload(&mut wrapper));
    std::fs::remove_dir_all(&held)?;
    if !control?.contains(SECRET) {
        lgwks_std::trace::warn!("cap: this host has no user manager that runs the control");
        return Ok(());
    }
    if confined?.contains(SECRET) {
        Err(DenyFault::ManagerRead)
    } else {
        Ok(())
    }
}
