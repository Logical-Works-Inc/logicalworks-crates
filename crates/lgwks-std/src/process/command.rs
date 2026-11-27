//! The one way to start a process (#1164, ADR 0092): `std`'s builder, with
//! every start owned.
//!
//! A raw `std::process::Command` starts a child that nothing owns once the
//! caller stops running. A SIGKILL of the caller, an OOM kill or a test
//! timeout leaves the child and everything it started alive, and the tests of
//! two consumers held a 24 GB Mac for hours that way (#1160). [`Command`] is the
//! same builder with one difference at the start: on Unix the child leads a
//! process group of its own, and the group is held on this process's lifeline
//! before [`Command::spawn`] returns. Then:
//!
//! - when the child's leader ends, its group is killed before the leader is
//!   reaped, so nothing it put in the background outlives it, and the group id
//!   cannot be reissued to a stranger while it is signalled;
//! - a [`Child`] dropped before its leader ended kills the group and reaps it;
//! - when this process ends by any path, SIGKILL included, the lifeline kills
//!   every group still held (ADR 0087), and the ledger answers for a lifeline
//!   killed first (ADR 0090).
//!
//! Bounds, stated: a child that calls `setsid` or `setpgid` leaves its group and
//! is not held, and the census names it (ADR 0082); between the fork and the
//! hold's write, about 100 µs, a SIGKILL of this process leaves the child. Off
//! Unix there is no lifeline: a dropped child's tree is stopped, and a killed
//! caller's is not.

use std::ffi::OsStr;
use std::io;
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{ExitStatus, Output, Stdio};

/// The started child and how it ends.
mod child;

pub use child::Child;
use child::trace_error;

/// A process to start, built as `std::process::Command` is, started owned.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// let listed = lgwks_std::process::Command::new("ls").arg("-l").output()?;
/// assert!(listed.status.success());
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct Command {
    /// The builder every start configures.
    inner: std::process::Command,
    /// Whether the caller chose each stream: [`Command::output`] captures the
    /// two outputs and closes the input only where the caller chose nothing.
    chosen: [bool; 3],
}

/// The stream a choice is recorded for.
const STDIN: usize = 0;
/// See [`STDIN`].
const STDOUT: usize = 1;
/// See [`STDIN`].
const STDERR: usize = 2;

impl Command {
    /// A command that runs `program`, found as `std` finds it.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("git");
    /// command.arg("status");
    /// ```
    pub fn new(program: impl AsRef<OsStr>) -> Self {
        Self {
            inner: std::process::Command::new(program),
            chosen: [false; 3],
        }
    }

    /// Add one argument.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("echo");
    /// command.arg("hello");
    /// ```
    pub fn arg(&mut self, arg: impl AsRef<OsStr>) -> &mut Self {
        self.inner.arg(arg);
        self
    }

    /// Add each argument in order.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("git");
    /// command.args(["status", "--short"]);
    /// ```
    pub fn args<I, S>(&mut self, args: I) -> &mut Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.inner.args(args);
        self
    }

    /// Set one variable for the child.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("env");
    /// command.env("LANG", "C");
    /// ```
    pub fn env(&mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> &mut Self {
        self.inner.env(key, value);
        self
    }

    /// Set each variable for the child.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("env");
    /// command.envs([("LANG", "C"), ("TZ", "UTC0")]);
    /// ```
    pub fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.inner.envs(vars);
        self
    }

    /// Keep one variable from the child.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("env");
    /// command.env_remove("HOME");
    /// ```
    pub fn env_remove(&mut self, key: impl AsRef<OsStr>) -> &mut Self {
        self.inner.env_remove(key);
        self
    }

    /// Start the child with no inherited variable.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("/usr/bin/env");
    /// command.env_clear().env("PATH", "/usr/bin");
    /// ```
    pub fn env_clear(&mut self) -> &mut Self {
        self.inner.env_clear();
        self
    }

    /// Run the child in `dir`.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("ls");
    /// command.current_dir("/tmp");
    /// ```
    pub fn current_dir(&mut self, dir: impl AsRef<Path>) -> &mut Self {
        self.inner.current_dir(dir);
        self
    }

    /// The child's standard input.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("cat");
    /// command.stdin(std::process::Stdio::piped());
    /// ```
    pub fn stdin(&mut self, stream: impl Into<Stdio>) -> &mut Self {
        self.inner.stdin(stream);
        self.mark(STDIN)
    }

    /// The child's standard output.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("ls");
    /// command.stdout(std::process::Stdio::null());
    /// ```
    pub fn stdout(&mut self, stream: impl Into<Stdio>) -> &mut Self {
        self.inner.stdout(stream);
        self.mark(STDOUT)
    }

    /// The child's standard error.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("ls");
    /// command.stderr(std::process::Stdio::null());
    /// ```
    pub fn stderr(&mut self, stream: impl Into<Stdio>) -> &mut Self {
        self.inner.stderr(stream);
        self.mark(STDERR)
    }

    /// Record that the caller chose stream `which`.
    fn mark(&mut self, which: usize) -> &mut Self {
        if let Some(slot) = self.chosen.get_mut(which) {
            *slot = true;
        }
        self
    }

    /// Whether the caller chose stream `which`.
    fn is_chosen(&self, which: usize) -> bool {
        matches!(self.chosen.get(which), Some(true))
    }

    /// The name the child sees as `argv[0]`.
    ///
    /// ```no_run
    /// let mut command = lgwks_std::process::Command::new("/bin/sh");
    /// command.arg0("my-worker");
    /// ```
    #[cfg(unix)]
    pub fn arg0(&mut self, name: impl AsRef<OsStr>) -> &mut Self {
        self.inner.arg0(name);
        self
    }

    /// Replace this process with the command. Nothing is started beside this
    /// process, so nothing is held; it returns only the error that stopped it.
    ///
    /// ```no_run
    /// let error = lgwks_std::process::Command::new("sleep").arg("1").exec();
    /// eprintln!("exec failed: {error}");
    /// ```
    #[cfg(unix)]
    pub fn exec(&mut self) -> io::Error {
        self.inner.exec()
    }

    /// Start the child, owned: on Unix it leads its own group, held on this
    /// process's lifeline before this returns.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let mut child = lgwks_std::process::Command::new("sleep").arg("1").spawn()?;
    /// let status = child.wait()?;
    /// assert!(status.success());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The start's error, or the hold's: a child that could not be held is
    /// killed and reaped, never returned unowned.
    pub fn spawn(&mut self) -> io::Result<Child> {
        self.inherit_unchosen();
        self.start()
    }

    /// Run the child to its end and return its status and both outputs. The
    /// outputs are captured and the input is closed, unless the caller chose
    /// them.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let said = lgwks_std::process::Command::new("echo").arg("hi").output()?;
    /// assert_eq!(said.stdout, b"hi\n");
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The start's, the hold's, a read's or the wait's error.
    pub fn output(&mut self) -> io::Result<Output> {
        if !self.is_chosen(STDIN) {
            self.inner.stdin(Stdio::null());
        }
        if !self.is_chosen(STDOUT) {
            self.inner.stdout(Stdio::piped());
        }
        if !self.is_chosen(STDERR) {
            self.inner.stderr(Stdio::piped());
        }
        self.start()?.wait_with_output()
    }

    /// Run the child to its end with the streams it inherits, and return its
    /// status.
    ///
    /// ```no_run
    /// # fn main() -> std::io::Result<()> {
    /// let status = lgwks_std::process::Command::new("true").status()?;
    /// assert!(status.success());
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The start's, the hold's or the wait's error.
    pub fn status(&mut self) -> io::Result<ExitStatus> {
        self.inherit_unchosen();
        self.start()?.wait()
    }

    /// Each stream the caller left alone is inherited, said on every start, so
    /// an earlier [`Command::output`] never changes a later start.
    fn inherit_unchosen(&mut self) {
        if !self.is_chosen(STDIN) {
            self.inner.stdin(Stdio::inherit());
        }
        if !self.is_chosen(STDOUT) {
            self.inner.stdout(Stdio::inherit());
        }
        if !self.is_chosen(STDERR) {
            self.inner.stderr(Stdio::inherit());
        }
    }

    /// Start the child in a group of its own and hold the group.
    #[cfg(unix)]
    fn start(&mut self) -> io::Result<Child> {
        let mut inner = self.inner.process_group(0).spawn()?;
        let held = i32::try_from(inner.id())
            .map_err(io::Error::other)
            .and_then(|pgid| super::hold_group(pgid).map(|hold| (pgid, hold)));
        match held {
            Ok((pgid, hold)) => Ok(Child::owned(inner, pgid, Some(hold))),
            Err(error) => {
                if let Err(unkilled) = inner.kill() {
                    trace_error(&unkilled, "a child that could not be held refused its kill");
                }
                if let Err(unreaped) = inner.wait() {
                    trace_error(&unreaped, "a child that could not be held was not reaped");
                }
                trace_error(&error, "spawn: a child that could not be held was stopped");
                Err(error)
            }
        }
    }

    /// Start the child; off Unix a group is its tree.
    #[cfg(not(unix))]
    fn start(&mut self) -> io::Result<Child> {
        let inner = self.inner.spawn()?;
        let root = i32::try_from(inner.id()).map_err(io::Error::other)?;
        Ok(Child::owned(inner, root, None))
    }
}
