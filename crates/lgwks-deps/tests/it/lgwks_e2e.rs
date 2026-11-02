//! The `lgwks` journey end to end: new, check, expand, run, one request.
//!
//! The suite drives the built binary the way each user does, in order, and
//! asserts what the user sees: the scaffolded files, the check verdict, the
//! expanded Rust, and one HTTP answer from the serving app over loopback.
//! Then it kills `lgwks run` alone, as a supervisor would, and requires the
//! port freed: the app must stop with the command that started it.

use std::error::Error;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use crate::lgwks_scratch::{
    Scratch, poll_ready, reference_app, run_lgwks, spawn_run, stop_run, test_port, unique_tag,
};
use crate::replay::refuse;

/// How long the app may outlive a killed `lgwks run`: the watchdog's `TERM`,
/// then its grace before `KILL`, with room to spare.
const STOP_DEADLINE: Duration = Duration::from_secs(30);

/// What this test returns.
type TestResult = Result<(), Box<dyn Error>>;

/// How long the test waits for cargo to build and the app to accept.
const SERVE_DEADLINE: Duration = Duration::from_secs(300);

/// How long the test rests between readiness probes.
const PROBE_REST: Duration = Duration::from_millis(200);

/// How long one HTTP exchange may take.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);

/// A unique app name for this run, so the cleanup kill matches only the
/// processes this test started.
fn app_name() -> Result<String, Box<dyn Error>> {
    unique_tag("e2e").map_err(|error| error.into())
}

/// Wait until `addr` accepts, or the deadline passes.
fn await_ready(addr: SocketAddr) -> TestResult {
    if poll_ready(SERVE_DEADLINE, PROBE_REST, || {
        TcpStream::connect(addr).is_ok()
    }) {
        Ok(())
    } else {
        Err("the app never accepted".into())
    }
}

/// POST `body` to the drafts route and answer the status with the raw reply.
fn post_drafts(port: u16, tag: &str, body: &[u8]) -> Result<(u16, Vec<u8>), Box<dyn Error>> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = TcpStream::connect(addr.as_str())
        .map_err(|error| format!("the drafts route could not be reached: {error}"))?;
    stream
        .set_write_timeout(Some(HTTP_TIMEOUT))
        .map_err(|error| format!("the write deadline could not be armed: {error}"))?;
    stream
        .set_read_timeout(Some(HTTP_TIMEOUT))
        .map_err(|error| format!("the read deadline could not be armed: {error}"))?;
    write!(
        stream,
        "POST /t/demo/drafts HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nIdempotency-Key: {tag}-1\r\nConnection: close\r\n\r\n",
        body.len(),
    )
    .map_err(|error| format!("the request could not be written: {error}"))?;
    stream
        .write_all(body)
        .map_err(|error| format!("the request body could not be written: {error}"))?;
    stream
        .flush()
        .map_err(|error| format!("the request could not be flushed: {error}"))?;
    let mut reply = Vec::new();
    stream
        .read_to_end(&mut reply)
        .map_err(|error| format!("the reply could not be read: {error}"))?;
    let head_end = reply
        .windows(b"\r\n\r\n".len())
        .position(|window| window == b"\r\n\r\n")
        .map(|at| at.saturating_add(4));
    let head_end = match head_end {
        Some(end) => end,
        None => return refuse("the reply carries no head"),
    };
    let (head, _) = reply.split_at(head_end);
    let status = match std::str::from_utf8(head) {
        Ok(head) => match head.lines().next() {
            Some(line) => line.to_owned(),
            None => return refuse("the reply carries no status"),
        },
        Err(_) => return refuse("the reply head is not text"),
    };
    let code = match status.split_whitespace().nth(1) {
        Some(code) => code.to_owned(),
        None => return refuse(format!("the status carries no code: {status}")),
    };
    match code.parse::<u16>() {
        Ok(code) => Ok((code, reply)),
        Err(_) => refuse(format!("the status code is not a number: {code}")),
    }
}

/// New, check, expand, run, one request, stop: the whole journey.
#[test]
fn new_check_expand_run_serves_one_request() -> TestResult {
    let scratch = Scratch::fresh("e2e").map_err(|error| format!("no scratch: {error}"))?;
    let name = app_name()?;
    let port = test_port()?;
    let reference = reference_app(scratch.dir(), name.as_str())?;
    if !reference.contains("serve on port") {
        return refuse("the scaffold holds no serve block");
    }
    let dir = scratch.dir().join(&name);
    let outcome = run_lgwks(&dir, &["check"])?;
    if outcome.code != Some(0) {
        return refuse(format!(
            "lgwks check refused the scaffold: {}",
            outcome.stderr
        ));
    }
    let outcome = run_lgwks(&dir, &["expand"])?;
    if outcome.code != Some(0) {
        return refuse(format!(
            "lgwks expand refused the scaffold: {}",
            outcome.stderr
        ));
    }
    if !String::from_utf8_lossy(outcome.stdout.as_slice()).contains("pub async fn drafts") {
        return refuse("the expansion holds no drafts flow");
    }
    let mut child = spawn_run(&dir, port, false)?;
    let outcome: Result<(), Box<dyn Error>> = (|| {
        await_ready(SocketAddr::from(([127, 0, 0, 1], port)))?;
        let (code, reply) = post_drafts(port, name.as_str(), b"[1,2]")?;
        if code != 200 {
            return refuse(format!("the drafts route answered {code}"));
        }
        if !String::from_utf8_lossy(reply.as_slice()).contains("[1,2]") {
            return refuse("the drafts route did not echo the body");
        }
        Ok(())
    })();
    let stopped = stop_run(&mut child, port, STOP_DEADLINE);
    outcome?;
    stopped?;
    assert_untracked(&dir)
}

/// The app directory holds the scaffold's two files and the lock `run`
/// resolved, nothing else: the build lands in the shared target.
fn assert_untracked(dir: &Path) -> TestResult {
    let mut names = Vec::new();
    let entries = std::fs::read_dir(dir)
        .map_err(|error| format!("the scaffold could not be listed: {error}"))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("a scaffold entry could not be read: {error}"))?;
        names.push(entry.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    let wanted = ["Cargo.lock", "Cargo.toml", "app.rs"];
    if names != wanted {
        return refuse(format!(
            "the app directory holds more than its scaffold and lock: {names:?}"
        ));
    }
    Ok(())
}
