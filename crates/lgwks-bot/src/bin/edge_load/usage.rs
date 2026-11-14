//! What a server process used, and what the host is.
//!
//! On Linux a process's numbers come from `/proc/{pid}`: `VmHWM` in `status` is
//! the kernel's own peak resident set, and fields 14 and 15 of `stat` are user
//! and system time in ticks of `USER_HZ`, which is 100 a second on every Linux
//! ABI. Elsewhere `ps -o rss=,time=` is read: its resident set is the current
//! one, so the rig samples it after each round and keeps the largest. Each row
//! names the method it used ([`RSS_METHOD`]).
//!
//! The host's memory is the cgroup's `memory.max` when one is set, so a run
//! under a 2 GB limit says 2 GB, and the machine's total otherwise.

use std::process::Command;

use crate::{LoadError, refused};

/// How the resident set on a row was read.
pub(super) const RSS_METHOD: &str = if cfg!(target_os = "linux") {
    "proc-vmhwm"
} else {
    "ps-sampled"
};

/// A server's CPU time so far and its resident set.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Usage {
    /// User plus system time, in microseconds.
    pub(super) cpu_micros: u64,
    /// The peak resident set (Linux) or the current one (elsewhere), in bytes.
    pub(super) rss_bytes: u64,
}

/// What `pid` has used so far.
pub(super) fn of(pid: u32) -> Result<Usage, LoadError> {
    if cfg!(target_os = "linux") {
        from_proc(pid)
    } else {
        from_ps(pid)
    }
}

/// `/proc/{pid}`'s peak resident set and CPU ticks.
fn from_proc(pid: u32) -> Result<Usage, LoadError> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status"))?;
    let kilobytes = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or(LoadError::Malformed("no VmHWM in /proc status"))?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    // The command name may hold spaces, so fields are counted from its `)`;
    // the first field after it is the third, so utime and stime are 11 and 12.
    let (_, fields) = stat
        .rsplit_once(')')
        .ok_or(LoadError::Malformed("no command in /proc stat"))?;
    let mut ticks = fields.split_whitespace().skip(11);
    let (Some(user), Some(system)) = (ticks.next(), ticks.next()) else {
        return refused(LoadError::Malformed("no CPU ticks in /proc stat"));
    };
    let (Ok(user), Ok(system)) = (user.parse::<u64>(), system.parse::<u64>()) else {
        return refused(LoadError::Malformed("CPU ticks are not numbers"));
    };
    Ok(Usage {
        cpu_micros: user.saturating_add(system).saturating_mul(10_000),
        rss_bytes: kilobytes.saturating_mul(1_024),
    })
}

/// `ps`'s current resident set and cumulative CPU time.
fn from_ps(pid: u32) -> Result<Usage, LoadError> {
    let text = output(
        Command::new("ps")
            .args(["-o", "rss=,time=", "-p"])
            .arg(pid.to_string()),
    )?;
    let mut words = text.split_whitespace();
    let (Some(kilobytes), Some(time)) = (words.next(), words.next()) else {
        return refused(LoadError::Malformed("ps named no such process"));
    };
    let Ok(kilobytes) = kilobytes.parse::<u64>() else {
        return refused(LoadError::Malformed(
            "ps answered a resident set that is not a number",
        ));
    };
    Ok(Usage {
        cpu_micros: cpu_time(time)?,
        rss_bytes: kilobytes.saturating_mul(1_024),
    })
}

/// `[[dd-]hh:]mm:ss[.ff]` in microseconds, as `ps` writes CPU time.
fn cpu_time(text: &str) -> Result<u64, LoadError> {
    let (days, clock) = match text.split_once('-') {
        Some((days, clock)) => (days, clock),
        None => ("0", text),
    };
    let (whole, fraction) = match clock.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (clock, ""),
    };
    let Ok(days) = days.parse::<u64>() else {
        return refused(LoadError::Malformed("a CPU time's days are not a number"));
    };
    let mut seconds = days.saturating_mul(86_400);
    let mut place = 1_u64;
    for part in whole.rsplit(':') {
        let Ok(value) = part.parse::<u64>() else {
            return refused(LoadError::Malformed("a CPU time's clock is not a number"));
        };
        seconds = seconds.saturating_add(value.saturating_mul(place));
        place = place.saturating_mul(60);
    }
    let mut micros = 0_u64;
    let mut scale = 100_000_u64;
    for digit in fraction.chars().take(6) {
        let digit = digit.to_digit(10).ok_or(LoadError::Malformed(
            "a CPU time's fraction is not a number",
        ))?;
        micros = micros.saturating_add(u64::from(digit).saturating_mul(scale));
        scale = scale.div_euclid(10);
    }
    Ok(seconds.saturating_mul(1_000_000).saturating_add(micros))
}

/// The host a run is on: what it can give one process.
#[derive(Clone, Debug)]
pub(super) struct Host {
    /// CPUs this process may run on.
    pub(super) cpus: usize,
    /// Memory this process may use, in bytes.
    pub(super) memory_bytes: u64,
}

/// The host this process runs on.
pub(super) fn host() -> Result<Host, LoadError> {
    let cpus = std::thread::available_parallelism()?.get();
    let memory_bytes = if cfg!(target_os = "linux") {
        linux_memory()?
    } else {
        let text = output(Command::new("sysctl").args(["-n", "hw.memsize"]))?;
        let Ok(bytes) = text.trim().parse::<u64>() else {
            return refused(LoadError::Malformed(
                "sysctl answered a size that is not a number",
            ));
        };
        bytes
    };
    Ok(Host { cpus, memory_bytes })
}

/// The cgroup's memory ceiling when it sets one, else `MemTotal`.
fn linux_memory() -> Result<u64, LoadError> {
    let group = std::fs::read_to_string("/proc/self/cgroup")?;
    let ceiling = group
        .lines()
        .find_map(|line| line.strip_prefix("0::"))
        .and_then(|path| std::fs::read_to_string(format!("/sys/fs/cgroup{path}/memory.max")).ok())
        .and_then(|max| max.trim().parse::<u64>().ok());
    if let Some(bytes) = ceiling {
        return Ok(bytes);
    }
    let info = std::fs::read_to_string("/proc/meminfo")?;
    info.lines()
        .find_map(|line| line.strip_prefix("MemTotal:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kilobytes| kilobytes.saturating_mul(1_024))
        .ok_or(LoadError::Malformed("no MemTotal in /proc/meminfo"))
}

/// The one-minute load average as `uptime` prints it.
pub(super) fn load_average() -> Result<String, LoadError> {
    let text = output(&mut Command::new("uptime"))?;
    let (_, after) = text
        .split_once("load average")
        .ok_or(LoadError::Malformed("uptime printed no load average"))?;
    after
        .split(|ch: char| ch.is_ascii_whitespace() || ch == ',')
        .find(|word| word.starts_with(|ch: char| ch.is_ascii_digit()))
        .map(str::to_owned)
        .ok_or(LoadError::Malformed("uptime printed no load number"))
}

/// The host's own name, as `uname -n` prints it.
pub(super) fn node_name() -> Result<String, LoadError> {
    Ok(output(Command::new("uname").arg("-n"))?.trim().to_owned())
}

/// The commit this tree is at, or `unknown` outside a git checkout.
pub(super) fn commit() -> String {
    let text = Command::new("git")
        .args(["rev-parse", "--short=12", "HEAD"])
        .output();
    match text {
        Ok(done) if done.status.success() => {
            String::from_utf8_lossy(&done.stdout).trim().to_owned()
        }
        Ok(done) => {
            lgwks_std::trace::warn!(status = %done.status, "edge-load: git named no commit");
            String::from("unknown")
        }
        Err(error) => {
            lgwks_std::trace::warn!(%error, "edge-load: git did not run");
            String::from("unknown")
        }
    }
}

/// What `command` printed, or a refusal naming its exit.
fn output(command: &mut Command) -> Result<String, LoadError> {
    let done = command.output()?;
    if !done.status.success() {
        return refused(LoadError::Malformed("a host command exited with a failure"));
    }
    let Ok(text) = String::from_utf8(done.stdout) else {
        return refused(LoadError::Malformed("a host command printed non-UTF-8"));
    };
    Ok(text)
}
