//! The `unshare` fallback against the target it confines. `user_namespaces(7)`:
//! a process that makes a user namespace holds every capability inside it, so
//! the fallback's mapped root could `umount` its own overmount unless the exec
//! drops them.

use std::io;
use std::path::Path;

use super::{UNSHARE, unshare_deny};

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
