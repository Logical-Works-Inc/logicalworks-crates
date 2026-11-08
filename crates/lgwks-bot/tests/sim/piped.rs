//! A shipped line-at-a-time binary, driven through its pipes.
//!
//! The tools that read one command per line and answer one JSON line are tried the
//! same way: start one, write to it, read what it said, sometimes kill it first.

use std::ffi::OsStr;
use std::io::{BufRead as _, BufReader, Write as _};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A test's outcome: setup failures propagate instead of panicking.
pub type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A built binary, read at run time because an archived run moves it between shards.
pub fn located(names: [&str; 2], built: &str) -> PathBuf {
    for name in names {
        if let Some(found) = std::env::var_os(name) {
            return found.into();
        }
    }
    built.into()
}

/// Feed `script` to one process and read every line it answered.
pub fn session<A: AsRef<OsStr>>(
    binary: &PathBuf,
    arguments: &[A],
    script: &str,
) -> Outcome<Vec<String>> {
    let mut child = Command::new(binary)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child
        .stdin
        .take()
        .ok_or("the child has a stdin")?
        .write_all(script.as_bytes())?;
    let done = child.wait_with_output()?;
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    Ok(String::from_utf8(done.stdout)?
        .lines()
        .map(str::to_owned)
        .collect())
}

/// A running process to feed and then kill, with the line it last answered.
pub struct Owner {
    /// The process.
    child: Child,
    /// Held open so the process keeps reading.
    input: ChildStdin,
    /// What it answers.
    reader: BufReader<ChildStdout>,
}

impl Owner {
    /// Start one, with its pipes taken.
    pub fn start<A: AsRef<OsStr>>(binary: &PathBuf, arguments: &[A]) -> Outcome<Self> {
        let mut child = Command::new(binary)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let input = child.stdin.take().ok_or("the owner has a stdin")?;
        let output = child.stdout.take().ok_or("the owner has a stdout")?;
        Ok(Self {
            child,
            input,
            reader: BufReader::new(output),
        })
    }

    /// Write `script`, and read one answer for each of its lines.
    pub fn answers(&mut self, script: &str) -> Outcome<Vec<String>> {
        self.input.write_all(script.as_bytes())?;
        self.input.flush()?;
        let mut answers = Vec::new();
        for _ in script.lines() {
            let mut line = String::new();
            self.reader.read_line(&mut line)?;
            answers.push(line);
        }
        Ok(answers)
    }

    /// Kill the process with no warning, and reap it.
    pub fn kill(mut self) -> Outcome {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }
}
