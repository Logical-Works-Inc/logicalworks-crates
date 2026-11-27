//! The lifeline: what this process holds is released when it ends, however it
//! ends (#1160, ADR 0087).
//!
//! A guard's `Drop` runs only while its owner runs code. A SIGKILL, an OOM kill
//! or a power cut runs none, and macOS has no kernel hook that ends a dead
//! parent's children. So the release must not need the owner: each hold is a
//! line written to a small `/bin/sh` process, the lifeline, which leads its
//! own process group and reads the lines on a pipe only this process holds the
//! write end of. When this process ends by any path the kernel closes that end,
//! the lifeline reads end of file, and it releases every hold still open:
//! groups first (`kill -9 -<pgid>`), then RAM disks (`hdiutil detach -force`),
//! then release programs. Then it exits.
//!
//! One lifeline serves the whole process. A lifeline that died while its owner
//! lives is started again at the next hold or release, and every open hold is
//! written to the new one first, so no hold is ever owed to a dead lifeline for
//! longer than the gap to that call.
//!
//! A lifeline and its owner can both die, the lifeline first: then the
//! [`ledger`] answers. Each hold is a line in the owner's lease record before
//! it returns, and the first hold of every later process sweeps the records
//! no live owner or lifeline holds.

mod ledger;

use std::collections::BTreeMap;
use std::io::{self, Write as _};
#[cfg(unix)]
use std::os::unix::process::CommandExt as _;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Mutex, MutexGuard};

/// The most holds one process keeps open. A hold past it is refused as
/// [`io::ErrorKind::QuotaExceeded`], never buffered.
const MAX_HOLDS: usize = 65_536;

/// The lifeline's program, as `argv[0]` names it, so a census can tell it from
/// a leaked shell.
pub(super) const LIFELINE_NAME: &str = "lgwks-lifeline";

/// The lifeline itself. Each hold is a variable `h_<id>`, so a release is one
/// `unset`; the id list is compacted when it is mostly released ids. Ids are
/// digits and values are checked by [`checked`], so `eval` only ever sees
/// digits and its own quoting.
///
/// One ordinary literal per line, never one raw string: a tool that reads Rust
/// a line at a time (the mutants gate's instrumenter, 2026-11-26) took the
/// `&&` and `$((n+1))` inside a multi-line raw string for Rust operators and
/// rewrote them, and every lifeline then ended at once with a shell syntax
/// error (status 2). A line that is one whole literal holds no operator.
const SCRIPT: &str = concat!(
    "trap '' HUP INT QUIT\n",
    "ids=; n=0; live=0\n",
    "while read -r op id kind value; do\n",
    "  case $id in ''|*[!0-9]*) continue ;; esac\n",
    "  case $op in\n",
    "    +) eval \"h_$id=\\\"\\$kind:\\$value\\\"\"; ids=\"$ids $id\"; n=$((n+1)); live=$((live+1)) ;;\n",
    "    -) eval \"unset h_$id\"; live=$((live-1))\n",
    "       if [ \"$n\" -gt 64 ] && [ \"$n\" -gt $((live*2)) ]; then\n",
    "         kept=; for i in $ids; do eval \"e=\\${h_$i-}\"; [ -n \"$e\" ] && kept=\"$kept $i\"; done\n",
    "         ids=$kept; n=$live\n",
    "       fi ;;\n",
    "  esac\n",
    "done\n",
    "for pass in g d r; do\n",
    "  for i in $ids; do\n",
    "    eval \"e=\\${h_$i-}\"; [ -n \"$e\" ] || continue\n",
    "    kind=${e%%:*}; value=${e#*:}\n",
    "    [ \"$kind\" = \"$pass\" ] || continue\n",
    "    case $kind in\n",
    "      g) kill -9 \"-$value\" ;;\n",
    "      d) /usr/bin/hdiutil detach \"$value\" -force ;;\n",
    "      r) \"${value%%,*}\" \"${value#*,}\" ;;\n",
    "    esac\n",
    "  done\n",
    "done >/dev/null 2>&1\n",
    "[ -z \"${LGWKS_LEASE_PATH-}\" ] || rm -f -- \"$LGWKS_LEASE_PATH\"\n",
);

/// What the lifeline does for one hold when this process ends.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Release {
    /// `kill -9 -<pgid>`.
    Group(i32),
    /// `hdiutil detach <device> -force`.
    Device(String),
    /// `<program> <argument>`.
    Run(String, String),
}

impl Release {
    /// The hold line for `id`.
    fn line(&self, id: u64) -> String {
        match *self {
            Self::Group(pgid) => format!("+ {id} g {pgid}\n"),
            Self::Device(ref device) => format!("+ {id} d {device}\n"),
            Self::Run(ref program, ref argument) => format!("+ {id} r {program},{argument}\n"),
        }
    }
}

/// The live lifeline: its process and the write end of its pipe.
struct Line {
    /// The `sh` process.
    child: Child,
    /// The pipe it reads; closing it is the release signal.
    stdin: ChildStdin,
}

/// Every hold open in this process, and the lifeline that knows them.
struct State {
    /// The lifeline, once a hold started one.
    line: Option<Line>,
    /// Each open hold by id.
    held: BTreeMap<u64, Release>,
    /// The next id.
    next: u64,
    /// This process's lease record, once a hold opened it.
    ledger: Option<ledger::Record>,
}

/// State whose scope the kernel sets, not a tenant: the kernel closes a pipe
/// when its process ends, so the holds a lifeline answers for are the whole
/// process's, and one per process is the only correct count. A tenant key
/// could split what one end of file releases. Held briefly, never across an
/// await.
struct KernelWide<T>(Mutex<T>);

impl<T> KernelWide<T> {
    /// The value, a poisoned lock recovered: every write to the map is one
    /// call, so a panic elsewhere leaves it whole.
    fn hold(&self) -> MutexGuard<'_, T> {
        match self.0.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// The one lifeline state of this process, every hold it has open.
static STATE: KernelWide<State> = KernelWide(Mutex::new(State {
    line: None,
    held: BTreeMap::new(),
    next: 1,
    ledger: None,
}));

/// The state, held.
fn state() -> MutexGuard<'static, State> {
    STATE.hold()
}

/// Kill and reap a lifeline this process no longer uses, naming a refusal.
fn stop(child: &mut Child) {
    if matches!(child.try_wait(), Ok(None))
        && let Err(error) = child.kill()
    {
        #[cfg(feature = "trace")]
        crate::trace::debug!(%error, "lifeline: a lifeline refused its kill");
        #[cfg(not(feature = "trace"))]
        drop(error);
    }
    if let Err(error) = child.wait() {
        #[cfg(feature = "trace")]
        crate::trace::debug!(%error, "lifeline: a lifeline could not be reaped");
        #[cfg(not(feature = "trace"))]
        drop(error);
    }
}

impl State {
    /// Whether the lifeline runs. An exited one is reaped here.
    fn live(&mut self) -> bool {
        let Some(line) = self.line.as_mut() else {
            return false;
        };
        match line.child.try_wait() {
            Ok(None) => true,
            // A lifeline that ends while its owner lives was killed from
            // outside: what it held is replayed to the next one, and the cause
            // is named, since nothing else sees it.
            Ok(Some(status)) => {
                #[cfg(feature = "trace")]
                crate::trace::warn!(%status, pid = line.child.id(), holds = self.held.len(), "lifeline: the lifeline ended while its owner lives");
                #[cfg(not(feature = "trace"))]
                let _ended = status;
                false
            }
            Err(error) => {
                #[cfg(feature = "trace")]
                crate::trace::warn!(%error, pid = line.child.id(), "lifeline: the lifeline could not be waited on");
                #[cfg(not(feature = "trace"))]
                drop(error);
                false
            }
        }
    }

    /// Write `text` to the lifeline, or start a fresh one that is told every
    /// open hold, which includes whatever `text` said.
    ///
    /// A pipe takes a write until its reader has exited, so a lifeline killed
    /// just before the write swallows the line. It is asked again after the
    /// write: one that died meanwhile is replaced and told every hold.
    fn send(&mut self, text: &str) -> io::Result<()> {
        if self.live()
            && let Some(line) = self.line.as_mut()
            && line.stdin.write_all(text.as_bytes()).is_ok()
            && self.live()
        {
            return Ok(());
        }
        self.restart()
    }

    /// Stop the old lifeline without releasing anything, then start a new one
    /// and replay every open hold to it. With nothing held none is started.
    fn restart(&mut self) -> io::Result<()> {
        if let Some(mut old) = self.line.take() {
            // SIGKILL before the pipe closes: a lifeline that read end of file
            // would release holds this process still owns.
            stop(&mut old.child);
            drop(old.stdin);
        }
        if self.held.is_empty() {
            return Ok(());
        }
        let mut line = spawn(self.ledger.as_ref())?;
        let replay: String = self
            .held
            .iter()
            .map(|(&id, release)| release.line(id))
            .collect();
        if let Err(error) = line.stdin.write_all(replay.as_bytes()) {
            stop(&mut line.child);
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, "lifeline: a fresh lifeline refused its replay");
            return Err(error);
        }
        self.line = Some(line);
        Ok(())
    }
}

/// Start a lifeline: `sh` as the leader of its own group, so a signal to this
/// process's group never reaches it, reading a pipe. It holds none of this
/// process's output: a lifeline on an inherited stderr kept nextest's capture
/// pipe open past the test's exit, a leak to nextest and a stall to any parent
/// that reads this process's output to its end. Its stdout is the lease
/// `record`, so it holds the record's lock while it lives, and it removes the
/// record once it has released.
fn spawn(record: Option<&ledger::Record>) -> io::Result<Line> {
    let mut command = Command::new("/bin/sh");
    command
        .arg0(LIFELINE_NAME)
        .args(["-c", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove(ledger::PATH_VAR)
        .process_group(0);
    if let Some(record) = record {
        command
            .stdout(record.share()?)
            .env(ledger::PATH_VAR, record.path());
    }
    let mut child = command.spawn()?;
    if let Some(stdin) = child.stdin.take() {
        return Ok(Line { child, stdin });
    }
    stop(&mut child);
    let refusal = io::Error::other("lifeline: the lifeline started without its pipe");
    #[cfg(feature = "trace")]
    crate::trace::debug!(%refusal, "spawn: returning an error to the caller");
    Err(refusal)
}

/// An open hold: released by the lifeline if this process ends first, let go
/// without any release when dropped.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// let hold = lgwks_std::process::hold_run(
///     std::path::Path::new("/bin/rm"),
///     std::path::Path::new("/tmp/my-lock"),
/// )?;
/// // ... work; a SIGKILL here still runs `/bin/rm /tmp/my-lock` ...
/// drop(hold); // the work ended: nothing is run
/// # Ok(())
/// # }
/// ```
#[must_use = "dropping a hold lets it go at once"]
#[derive(Debug)]
pub struct LifelineHold {
    /// The id the lifeline knows it by.
    id: u64,
}

impl Drop for LifelineHold {
    fn drop(&mut self) {
        let mut state = state();
        if state.held.remove(&self.id).is_none() {
            return;
        }
        let text = format!("- {}\n", self.id);
        if let Err(error) = state.send(&text) {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, id = self.id, "lifeline: a release could not be written; the next hold restarts the lifeline");
            #[cfg(not(feature = "trace"))]
            drop(error);
        }
        // The lifeline let go first, so a crash between the two leaves an open
        // lease, which a sweep releases again, never a hold no one records.
        let moved = match state.ledger.as_mut().map(|record| record.remove(self.id)) {
            Some(Ok(moved)) => moved,
            Some(Err(error)) => {
                #[cfg(feature = "trace")]
                crate::trace::warn!(%error, id = self.id, "lifeline: a let-go lease could not be recorded");
                #[cfg(not(feature = "trace"))]
                drop(error);
                false
            }
            None => false,
        };
        if moved && let Err(error) = state.restart() {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, "lifeline: a lifeline on the rewritten record could not start; the next hold retries");
            #[cfg(not(feature = "trace"))]
            drop(error);
        }
    }
}

/// What the ledger records for `release`: a group by its leader's identity,
/// so a sweep never signals a stranger that took the pid. A group whose
/// leader is already gone has no identity to check, and only the lifeline
/// holds it.
fn lease_of(release: &Release) -> io::Result<Option<ledger::Lease>> {
    Ok(match *release {
        Release::Group(pgid) => super::identify_process(pgid)?.map(ledger::Lease::Group),
        Release::Device(ref device) => Some(ledger::Lease::Device {
            device: device.clone(),
            image: ledger::image_of(device),
        }),
        Release::Run(ref program, ref argument) => {
            Some(ledger::Lease::Run(program.clone(), argument.clone()))
        }
    })
}

/// An `io::Error` of `kind` saying `why`, logged where it is made.
fn refuse(kind: io::ErrorKind, why: String) -> io::Error {
    let refusal = io::Error::new(kind, why);
    #[cfg(feature = "trace")]
    crate::trace::debug!(%refusal, "lifeline: returning an error to the caller");
    refusal
}

/// Open a hold, writing it to the lifeline before returning.
fn hold(release: Release) -> io::Result<LifelineHold> {
    hold_within(release, MAX_HOLDS)
}

/// [`hold`] under a bound of `most` open holds, so a test reaches the bound
/// without opening 65,536 of them.
fn hold_within(release: Release, most: usize) -> io::Result<LifelineHold> {
    let lease = lease_of(&release)?;
    let mut state = state();
    if state.held.len() >= most {
        #[cfg(feature = "trace")]
        crate::trace::warn!(most, "lifeline: a hold past the bound is refused");
        return Err(refuse(
            io::ErrorKind::QuotaExceeded,
            format!("lifeline: {most} holds are open"),
        ));
    }
    let id = state.next;
    let Some(next) = id.checked_add(1) else {
        #[cfg(feature = "trace")]
        crate::trace::warn!(id, "lifeline: hold ids ran out");
        return Err(refuse(
            io::ErrorKind::QuotaExceeded,
            "lifeline: hold ids ran out".to_owned(),
        ));
    };
    state.next = next;
    // The first hold of a process sweeps what dead owners left, then opens
    // this process's record; the lease is on disk before the hold returns.
    let record = match state.ledger {
        Some(ref mut record) => record,
        None => state.ledger.insert(ledger::Record::open()?),
    };
    let moved = match lease {
        Some(lease) => record.add(id, lease)?,
        None => false,
    };
    let text = release.line(id);
    state.held.insert(id, release);
    let sent = if moved {
        state.restart()
    } else {
        state.send(&text)
    };
    if let Err(error) = sent {
        state.held.remove(&id);
        if let Some(record) = state.ledger.as_mut()
            && let Err(unrecorded) = record.remove(id)
        {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%unrecorded, "hold: a refused hold's lease could not be let go");
            #[cfg(not(feature = "trace"))]
            drop(unrecorded);
        }
        #[cfg(feature = "trace")]
        crate::trace::debug!(%error, "hold: returning an error to the caller");
        return Err(error);
    }
    Ok(LifelineHold { id })
}

/// `text` when it is not empty and every byte is one the lifeline's line and
/// `eval` read as is: letters, digits and `_ . / + -`.
fn checked(text: Option<&str>, what: &str) -> io::Result<String> {
    match text {
        Some(plain)
            if !plain.is_empty()
                && plain
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_./+-".contains(&byte)) =>
        {
            Ok(plain.to_owned())
        }
        other => Err(refuse(
            io::ErrorKind::InvalidInput,
            format!("lifeline: {what} {other:?} holds a character a lifeline line cannot carry"),
        )),
    }
}

/// Hold process group `pgid`: if this process ends before the hold is
/// dropped, the lifeline sends the group SIGKILL.
///
/// Drop the hold only once the group is gone or no longer this process's to
/// stop: a dropped hold is never acted on.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// use std::os::unix::process::CommandExt as _;
/// let child = std::process::Command::new("sleep").arg("60").process_group(0).spawn()?;
/// let pgid = i32::try_from(child.id()).map_err(std::io::Error::other)?;
/// let hold = lgwks_std::process::hold_group(pgid)?;
/// // A SIGKILL of this process now still ends the group.
/// lgwks_std::process::kill_process_group(pgid)?;
/// drop(hold);
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// `InvalidInput` for a pgid of 0 or less, `QuotaExceeded` past 65,536 open holds,
/// and the spawn or pipe error of a lifeline that could not be started.
pub fn hold_group(pgid: i32) -> io::Result<LifelineHold> {
    if pgid <= 0 {
        #[cfg(feature = "trace")]
        crate::trace::debug!(pgid, "lifeline: a hold of no process group is refused");
        return Err(refuse(
            io::ErrorKind::InvalidInput,
            format!("lifeline: {pgid} is not a process group id"),
        ));
    }
    hold(Release::Group(pgid))
}

/// Hold the macOS disk image `device` (`/dev/diskN`): if this process ends
/// before the hold is dropped, the lifeline detaches it with `-force`. A RAM
/// disk held this way gives its memory back when its owner dies.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// let hold = lgwks_std::process::hold_device("/dev/disk9")?;
/// // ... use the RAM disk; detach it yourself, then:
/// drop(hold);
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// `InvalidInput` for a name that is not `/dev/disk` and digits, `QuotaExceeded`
/// past 65,536 open holds, and a lifeline start's error.
pub fn hold_device(device: &str) -> io::Result<LifelineHold> {
    match device.strip_prefix("/dev/disk") {
        Some(digits) if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) => {
            hold(Release::Device(device.to_owned()))
        }
        _ => Err(refuse(
            io::ErrorKind::InvalidInput,
            format!("lifeline: {device:?} is not a /dev/diskN device"),
        )),
    }
}

/// Hold a release program: if this process ends before the hold is dropped,
/// the lifeline runs `program argument`, after every held group is killed and
/// every held device detached.
///
/// `program` must be absolute. Both are limited to letters, digits and
/// `_ . / + -`, the bytes a lifeline line carries unquoted.
///
/// ```no_run
/// # fn main() -> std::io::Result<()> {
/// use std::path::Path;
/// let hold = lgwks_std::process::hold_run(Path::new("/bin/rm"), Path::new("/tmp/job.lock"))?;
/// drop(hold);
/// # Ok(())
/// # }
/// ```
///
/// # Errors
///
/// `InvalidInput` for a relative program or a byte outside that set,
/// `QuotaExceeded` past 65,536 open holds, and a lifeline start's error.
pub fn hold_run(program: &Path, argument: &Path) -> io::Result<LifelineHold> {
    if !program.is_absolute() {
        #[cfg(feature = "trace")]
        crate::trace::debug!(?program, "lifeline: a relative release program is refused");
        return Err(refuse(
            io::ErrorKind::InvalidInput,
            format!("lifeline: release program {program:?} is not an absolute path"),
        ));
    }
    let program = checked(program.to_str(), "release program")?;
    let argument = checked(argument.to_str(), "release argument")?;
    hold(Release::Run(program, argument))
}

/// The pid of this process's running lifeline, if a hold started one.
///
/// A subreaper sweep passes over it: it is this process's child for as long
/// as this process lives, and it is never an orphan to collect.
///
/// ```no_run
/// let lifeline: Option<i32> = lgwks_std::process::lifeline_pid();
/// assert!(lifeline.is_none_or(|pid| pid > 0));
/// ```
#[must_use]
pub fn lifeline_pid() -> Option<i32> {
    let mut state = state();
    if !state.live() {
        return None;
    }
    let id = state.line.as_ref().map(|line| line.child.id())?;
    match i32::try_from(id) {
        Ok(pid) => Some(pid),
        Err(error) => {
            #[cfg(feature = "trace")]
            crate::trace::debug!(%error, id, "lifeline_pid: a pid past pid_t names no process");
            #[cfg(not(feature = "trace"))]
            drop(error);
            None
        }
    }
}

#[cfg(test)]
mod sim_lifeline_bounds;

#[cfg(test)]
mod tests {
    use super::{Release, checked};

    /// The line grammar the script reads: op, id, kind, value.
    #[test]
    fn each_release_is_one_line_the_lifeline_reads() {
        assert_eq!(Release::Group(42).line(7), "+ 7 g 42\n");
        assert_eq!(
            Release::Device("/dev/disk9".into()).line(8),
            "+ 8 d /dev/disk9\n"
        );
        assert_eq!(
            Release::Run("/bin/rm".into(), "/tmp/x".into()).line(9),
            "+ 9 r /bin/rm,/tmp/x\n"
        );
    }

    /// proc(5): `cmdline` is `argv` joined by NUL bytes, so a census reads the
    /// lifeline by its first field and never by a later argument that happens
    /// to match.
    #[test]
    fn a_census_knows_a_lifeline_by_its_first_argument_only() {
        use crate::process::orphans::is_lifeline_cmdline;
        assert!(is_lifeline_cmdline(b"lgwks-lifeline\0-c\0trap\0"));
        assert!(!is_lifeline_cmdline(b"sh\0-c\0lgwks-lifeline\0"));
        assert!(!is_lifeline_cmdline(b"lgwks-lifeline-x\0"));
        assert!(!is_lifeline_cmdline(b""));
    }

    /// A space, a separator, a glob or a quote would split or expand a line.
    #[test]
    fn a_value_the_line_cannot_carry_is_refused() {
        for bad in [
            "", "a b", "a,b", "a:b", "a*", "a'b", "a\"b", "$x", "a\nb", "a;b",
        ] {
            assert!(
                checked(Some(bad), "value").is_err(),
                "{bad:?} must be refused"
            );
        }
        assert!(
            checked(None, "value").is_err(),
            "a non-UTF-8 path is refused"
        );
        for good in ["/tmp/lgwks-gate-0193.lock", "/bin/mkdir", "a_b+c-d.e"] {
            assert!(
                checked(Some(good), "value").is_ok(),
                "{good:?} must be carried"
            );
        }
    }
}
