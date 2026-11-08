//! Deny-shaped confinement: the wrappers that hide a subtree from a child.

use std::io;
#[cfg(target_os = "linux")]
use std::path::Path;

use lgwks_deps::tokio::process::Command;

use super::DenyBeneath;
#[cfg(target_os = "macos")]
use super::{SandboxProfile, sandbox};

#[cfg(all(test, target_os = "linux"))]
mod sim_deny;

/// Deny `denied`'s subtrees however the platform allows: a sandbox profile
/// on macOS, an empty `tmpfs` overmounted on each subtree in a mount
/// namespace on Linux.
pub(super) fn deny(denied: &DenyBeneath) -> io::Result<Command> {
    for path in denied.paths() {
        let resolved = std::fs::canonicalize(path).map_err(|source| {
            io::Error::new(
                source.kind(),
                format!(
                    "a denied subtree is resolved before it is overmounted: {}: {source}",
                    path.display()
                ),
            )
        })?;
        if !resolved.is_dir() {
            lgwks_std::trace::warn!(?path, "a denial names a non-directory");
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("a denied path is a directory: {}", path.display()),
            ));
        }
    }
    deny_platform(denied)
}

/// Deny `denied`'s subtrees in a sandbox profile (macOS): allow by default,
/// deny reads beneath each resolved denial.
#[cfg(target_os = "macos")]
fn deny_platform(denied: &DenyBeneath) -> io::Result<Command> {
    let mut profile = SandboxProfile::allow_default();
    for path in denied.paths() {
        profile.deny_read_beneath(path)?;
    }
    profile.validate()?;
    sandbox(&profile)
}

/// Deny `denied`'s subtrees under an empty `tmpfs` in a mount namespace
/// (Linux): `bwrap` where the host ships it, else `unshare -m` with a shell
/// that overmounts each denial before it execs the target.
#[cfg(target_os = "linux")]
fn deny_platform(denied: &DenyBeneath) -> io::Result<Command> {
    let wrapper = deny_wrapper(denied.paths())?;
    mount_namespace_available()?;
    Ok(Command::from(wrapper))
}

/// The host's mount-namespace wrapper over `paths`, as a std command the
/// caller appends the target to: `bwrap` where the host ships it, else
/// `unshare`.
#[cfg(target_os = "linux")]
fn deny_wrapper<'a>(paths: impl Iterator<Item = &'a Path>) -> io::Result<std::process::Command> {
    if Path::new(BWRAP).exists() {
        return Ok(bwrap_deny(paths));
    }
    if Path::new(UNSHARE).exists() {
        return Ok(unshare_deny(paths));
    }
    lgwks_std::trace::warn!("no mount-namespace wrapper on this host");
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "lgwks_bot: denying a subtree on Linux needs bwrap or unshare",
    ))
}

/// Whether this host lets an unprivileged process open the mount namespace a
/// denial needs, asked once per process.
///
/// A container without `CAP_SYS_ADMIN` whose seccomp profile also refuses user
/// namespaces cannot overmount anything: the wrapper exits before the target
/// runs, which is fail-closed but reads as the child's own failure. The probe
/// overmounts the temp directory in a namespace of its own and runs `true`, so
/// that host gets an `Unsupported` refusal before the fork instead.
#[cfg(target_os = "linux")]
fn mount_namespace_available() -> io::Result<()> {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let available = *AVAILABLE.get_or_init(|| {
        let probe = std::env::temp_dir();
        let mut command = match deny_wrapper(std::iter::once(probe.as_path())) {
            Ok(command) => command,
            Err(error) => {
                lgwks_std::trace::warn!(%error, "no mount-namespace wrapper to probe");
                return false;
            }
        };
        command
            .arg("/bin/true")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match command.status() {
            Ok(status) => status.success(),
            Err(error) => {
                lgwks_std::trace::warn!(%error, "the mount-namespace probe could not run");
                false
            }
        }
    });
    if available {
        Ok(())
    } else {
        lgwks_std::trace::warn!("this host cannot open a mount namespace");
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "lgwks_bot: denying a subtree needs a mount namespace this host refuses",
        ))
    }
}

/// Every other target has no denial wrapper.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn deny_platform(_denied: &DenyBeneath) -> io::Result<Command> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "lgwks_bot: denying a subtree is supported on macOS and Linux only",
    ))
}

/// The `bwrap` that overmounts denials, and the `unshare` fallback. Text, not
/// a `Path`: `Path::new` is not callable in a const on the pinned toolchain.
#[cfg(target_os = "linux")]
const BWRAP: &str = "/usr/bin/bwrap";

/// The `unshare` that opens a mount namespace when `bwrap` is absent.
#[cfg(target_os = "linux")]
const UNSHARE: &str = "/usr/bin/unshare";

/// `bwrap --bind / / --tmpfs <denied>... --`: the child sees its own root
/// except for the denied subtrees, which read as empty directories.
#[cfg(target_os = "linux")]
fn bwrap_deny<'a>(paths: impl Iterator<Item = &'a Path>) -> std::process::Command {
    let mut command = std::process::Command::new(BWRAP);
    command.arg("--bind").arg("/");
    command.arg("/");
    for path in paths {
        command.arg("--tmpfs").arg(path.as_os_str());
    }
    command.arg("--");
    command
}

/// `unshare -m -r sh -c <overmount, then exec>`: the shell the namespace opens
/// overmounts each denial from `LGWKS_DENY_<n>` before it execs the target.
/// `-r` opens a user namespace beside the mount namespace and maps the caller
/// to root inside it, so an unprivileged supervisor can mount there too.
///
/// The denied paths travel in the environment rather than in the script, so
/// no quoting can misread them. A mount the kernel refuses exits the shell
/// before the target runs, which is fail-closed: the lane never runs
/// unconfined, it fails.
///
/// The mapping that lets the shell mount also makes it root over that
/// namespace, and a target that kept those capabilities could `umount` the
/// overmount and read what it hid. So the shell execs the target through
/// `setpriv`, which empties the bounding, inheritable and ambient sets and sets
/// no-new-privs: an exec as mapped root then gains nothing. A host with no
/// `setpriv` fails the exec, which is fail-closed the same way.
#[cfg(target_os = "linux")]
fn unshare_deny<'a>(paths: impl Iterator<Item = &'a Path>) -> std::process::Command {
    const SCRIPT: &str = r#"index=0
while :; do
  eval "denied=\${LGWKS_DENY_$index-}"
  [ -z "$denied" ] && break
  if [ -e "$denied" ] || [ -L "$denied" ]; then
    mount -t tmpfs -o size=64M,mode=0755 tmpfs "$denied" || exit 99
  fi
  index=$((index+1))
done
exec /usr/bin/setpriv --no-new-privs --bounding-set=-all --inh-caps=-all --ambient-caps=-all -- "$@""#;
    let mut command = std::process::Command::new(UNSHARE);
    command.arg("-m");
    command.arg("-r");
    command.arg("sh");
    command.arg("-c");
    command.arg(SCRIPT);
    command.arg("lgwks-deny");
    for (index, path) in paths.enumerate() {
        command.env(format!("LGWKS_DENY_{index}"), path.as_os_str());
    }
    command
}
