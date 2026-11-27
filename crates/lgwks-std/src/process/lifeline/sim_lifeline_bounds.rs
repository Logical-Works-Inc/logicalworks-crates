//! The lifeline against a saboteur, a flood and a rewritten script (#1160).
//!
//! Each actor drives the real lifeline: a killer SIGKILLs it from outside, a
//! flooder takes holds to the bound and past it, and the script itself is run
//! by `sh` with a held and a let-go release. The oracle is the rule each one
//! breaks, never the code's own answer.

use super::{Release, SCRIPT, hold_within, ledger, lifeline_pid, state};
use std::io;
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Stdio};

/// A churner: a hold that writes no lease line and its release move no
/// record, so they restart no lifeline; a flood of holds and releases past the
/// record's line bound rewrites it with only its open leases, and the lifeline
/// moves to the new file once. A restart where nothing moved would end every
/// lifeline a busy owner has at each hold; a rewrite that lost a lease would
/// leave it to no sweep.
#[test]
fn sim_lifeline_record_rewrite_keeps_the_open_leases_and_moves_once() -> io::Result<()> {
    let kept = hold_within(inert(), 8)?;
    let first = lifeline_pid().ok_or_else(|| io::Error::other("a hold starts a lifeline"))?;
    // A group whose leader is gone has no identity, so the ledger gets no line.
    let mut leader = Command::new("/usr/bin/true").process_group(0).spawn()?;
    let pgid = i32::try_from(leader.id()).map_err(io::Error::other)?;
    leader.wait()?;
    drop(hold_within(Release::Group(pgid), 8)?);
    let unmoved = lifeline_pid();
    // `kept` wrote one line, so the rewrite lands on the last hold here.
    // Each turn writes two lines, so half the bound reaches it.
    for _ in 0..ledger::COMPACT_AT >> 1_u32 {
        drop(hold_within(inert(), 8)?);
    }
    let moved = lifeline_pid();
    let path = state()
        .ledger
        .as_ref()
        .map(|record| record.path().to_owned())
        .ok_or_else(|| io::Error::other("a hold opens a record"))?;
    let text = std::fs::read_to_string(&path)?;
    drop(kept);
    let folded = ledger::fold(&text);
    assert_eq!(
        unmoved,
        Some(first),
        "a hold and release that moved no record restarted the lifeline"
    );
    assert!(
        moved.is_some_and(|pid| pid != first),
        "the rewritten record kept the old lifeline, which holds the old file's lock"
    );
    assert_eq!(
        folded.leases.len(),
        1,
        "the rewrite keeps exactly the open lease: {text}"
    );
    assert!(
        text.lines().count() < 16,
        "the rewrite kept let-go leases: {text}"
    );
    Ok(())
}

/// A release under a directory no host has, so a lifeline that ends with it
/// held runs nothing.
fn inert() -> Release {
    Release::Run("/nonexistent-lgwks-unit/x".to_owned(), "y".to_owned())
}

/// The script as `sh` reads it, escapes and all: at end of file a held release
/// runs, a let-go one does not, and `sh` ends with status 0. On 2026-11-26 a
/// rewritten script ended with status 2 before any release.
#[test]
fn sim_lifeline_script_runs_what_is_held_at_end_of_file() -> io::Result<()> {
    use std::io::Write as _;
    let dir = std::env::temp_dir().join(format!("lgwks-lifeline-unit-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    let (held, gone) = (dir.join("held"), dir.join("gone"));
    let lines = format!(
        "+ 1 r /bin/mkdir,{}\n+ 2 r /bin/mkdir,{}\n- 2\n",
        held.display(),
        gone.display()
    );
    let mut sh = Command::new("/bin/sh")
        .args(["-c", SCRIPT])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = sh
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("the script started without its pipe"))?;
    stdin.write_all(lines.as_bytes())?;
    drop(stdin);
    let out = sh.wait_with_output()?;
    let (ran, skipped) = (held.is_dir(), !gone.exists());
    std::fs::remove_dir_all(&dir)?;
    assert!(
        out.status.success(),
        "the script failed ({}): {}",
        out.status,
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(ran, "a held release did not run");
    assert!(skipped, "a let-go release ran");
    Ok(())
}

/// The bound at its edge: the holds up to it are taken, the next is refused as
/// a quota and never buffered, and a hold let go makes room.
#[test]
fn sim_lifeline_hold_past_the_bound_is_refused() -> io::Result<()> {
    let first = hold_within(inert(), 2)?;
    let second = hold_within(inert(), 2)?;
    let refused = hold_within(inert(), 2).err().map(|error| error.kind());
    drop(second);
    let room = hold_within(inert(), 2)?;
    drop((first, room));
    assert_eq!(refused, Some(io::ErrorKind::QuotaExceeded));
    Ok(())
}

/// A lifeline killed from outside is seen as gone, and the next hold starts a
/// new one: a dead lifeline must never read as live, or every later hold is
/// written to a process that releases nothing.
#[test]
fn sim_lifeline_killed_from_outside_is_seen_as_gone_and_replaced() -> io::Result<()> {
    let first = hold_within(inert(), 8)?;
    let old = lifeline_pid().ok_or_else(|| io::Error::other("a hold starts a lifeline"))?;
    crate::process::kill_process(old)?;
    let mut gone = false;
    for _ in 0..(1_u32 << 22) {
        if lifeline_pid().is_none() {
            gone = true;
            break;
        }
        std::thread::yield_now();
    }
    let second = hold_within(inert(), 8)?;
    let new = lifeline_pid();
    drop((first, second));
    assert!(gone, "a SIGKILLed lifeline still read as live");
    assert!(
        new.is_some_and(|pid| pid != old),
        "the next hold started no new lifeline"
    );
    Ok(())
}
