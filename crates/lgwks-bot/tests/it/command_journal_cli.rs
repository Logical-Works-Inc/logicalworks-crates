//! The shipped `command-journal` binary under a real kill, over a real socket.
//!
//! Source: #898's kill row: "kill -9 between the write and the reply: the request
//! fails, the write stands, the retry by idempotency key finds it". The levels past
//! 100 run in-process in `sim_command`, because a client here holds a socket and a
//! thread.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Condvar, Mutex};

use crate::sim::piped::{Outcome, located};
use crate::sim::scratch_file::ScratchFile;

/// The binary under test.
fn binary() -> PathBuf {
    located(
        [
            "NEXTEST_BIN_EXE_command-journal",
            "NEXTEST_BIN_EXE_command_journal",
        ],
        env!("CARGO_BIN_EXE_command-journal"),
    )
}

/// A directory of one test's own, gone when the test is.
struct Scratch(ScratchFile);

impl Scratch {
    /// A fresh directory path.
    fn new(name: &str) -> Outcome<Self> {
        Ok(Self(ScratchFile::new(name)?))
    }

    /// Where it is.
    fn path(&self) -> &Path {
        self.0.path()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        drop(std::fs::remove_dir_all(self.0.path()));
    }
}

/// A running server.
struct Server {
    /// The process.
    child: Child,
    /// Held open so it keeps serving; closing it stops the server.
    input: Option<ChildStdin>,
    /// What it prints.
    output: BufReader<ChildStdout>,
    /// Where it listens.
    address: String,
}

impl Server {
    /// Start one over `directory`, stalling after a write of `stall` if it is named.
    fn start(directory: &Path, stall: Option<&str>) -> Outcome<Self> {
        let mut command = Command::new(binary());
        command
            .arg(directory)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(key) = stall {
            command.args(["--stall-after-write", key]);
        }
        let mut child = command.spawn()?;
        let input = child.stdin.take().ok_or("the server has a stdin")?;
        let mut output = BufReader::new(child.stdout.take().ok_or("the server has a stdout")?);
        let mut line = String::new();
        output.read_line(&mut line)?;
        let address = line
            .trim()
            .strip_prefix("listening ")
            .ok_or("the server names its address")?
            .to_owned();
        Ok(Self {
            child,
            input: Some(input),
            output,
            address,
        })
    }

    /// Kill it with no warning, and reap it.
    fn kill(&mut self) -> Outcome {
        self.child.kill()?;
        self.child.wait()?;
        Ok(())
    }

    /// Close its stdin and require a clean exit.
    fn close(&mut self) -> Outcome {
        drop(self.input.take());
        let status = self.child.wait()?;
        assert!(status.success(), "{status}");
        Ok(())
    }

    /// Close its stdin and require a clean exit, done with it.
    fn stop(mut self) -> Outcome {
        self.close()
    }

    /// One request under `tenant`'s own key.
    fn request(
        &self,
        tenant: &str,
        (method, path): (&str, &str),
        body: &str,
    ) -> Outcome<(u16, String)> {
        let key = format!("key-{tenant}");
        exchange(&self.address, tenant, &key, (method, path), body)
    }

    /// `tenant`'s count.
    fn count(&self, tenant: &str) -> Outcome<(u16, String)> {
        self.request(tenant, ("GET", "/count"), "")
    }

    /// One keyed write under `tenant`'s own key.
    fn write(&self, tenant: &str, key: &str, value: u64) -> Outcome<(u16, String)> {
        self.request(tenant, ("POST", "/write"), &keyed(key, value))
    }
}

/// One request on a fresh connection, read to its end.
fn exchange(
    address: &str,
    tenant: &str,
    key: &str,
    (method, path): (&str, &str),
    body: &str,
) -> Outcome<(u16, String)> {
    let mut stream = TcpStream::connect(address)?;
    write!(
        stream,
        "{method} /t/{tenant}{path} HTTP/1.1\r\nhost: {address}\r\nauthorization: Bearer {key}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n{body}",
        body.len()
    )?;
    let mut raw = String::new();
    stream.read_to_string(&mut raw)?;
    let (head, rest) = raw.split_once("\r\n\r\n").ok_or("an HTTP response")?;
    let code = head.strip_prefix("HTTP/1.1 ").ok_or("an HTTP status")?;
    let status = code.get(..3).ok_or("a three digit status")?.parse()?;
    Ok((status, rest.to_owned()))
}

/// A keyed write's body.
fn keyed(key: &str, value: u64) -> String {
    format!("{{\"key\":\"{key}\",\"value\":{value}}}")
}

/// An answer with `status` and `body`.
fn said(status: u16, body: &str) -> (u16, String) {
    (status, body.to_owned())
}

/// What a held write's two sides report, in the order they happen.
enum Heard {
    /// The server printed a line, or its output failed.
    Line(std::io::Result<String>),
    /// The client got an answer with this status, or `None` when its request failed.
    Answer(Option<u16>),
}

/// The first report of a held write; a later one is never read.
#[derive(Default)]
struct First {
    /// The report, once one came.
    heard: Mutex<Option<Heard>>,
    /// Rung when it comes.
    bell: Condvar,
}

impl First {
    /// Report `said`, unless something came first.
    fn tell(&self, said: Heard) {
        match self.heard.lock() {
            Ok(mut slot) => {
                if slot.is_none() {
                    *slot = Some(said);
                }
            }
            Err(cause) => lgwks_std::trace::warn!(%cause, "First::tell: the report lock broke"),
        }
        self.bell.notify_all();
    }

    /// Wait for the first report.
    fn wait(&self) -> Outcome<Heard> {
        let broke = |cause: String| format!("the report lock broke: {cause}");
        let mut slot = self
            .heard
            .lock()
            .map_err(|cause| broke(cause.to_string()))?;
        loop {
            if let Some(heard) = slot.take() {
                return Ok(heard);
            }
            slot = self
                .bell
                .wait(slot)
                .map_err(|cause| broke(cause.to_string()))?;
        }
    }
}

/// The server's next line, unless the client is answered first. A write that should be
/// held was answered instead, so the server is killed and nothing waits on a line it
/// will never print: a broken hold fails in milliseconds, not at a timeout.
fn said_unless_answered(server: &mut Server, first: &First) -> Outcome<String> {
    let Server {
        ref mut child,
        ref mut output,
        ..
    } = *server;
    std::thread::scope(|scope| -> Outcome<String> {
        scope.spawn(move || {
            let mut line = String::new();
            let got = output.read_line(&mut line).map(|_| line.trim().to_owned());
            first.tell(Heard::Line(got));
        });
        match first.wait()? {
            Heard::Line(got) => Ok(got?),
            Heard::Answer(status) => {
                child.kill()?;
                child.wait()?;
                Err(format!("the held write was answered ({status:?}) before it synced").into())
            }
        }
    })
}

/// Send `key` and wait until the stalled server has synced it, then run `then`; whether
/// the client's request failed.
fn held_then(
    server: &mut Server,
    key: &str,
    then: impl FnOnce(&mut Server) -> Outcome,
) -> Outcome<bool> {
    let address = server.address.clone();
    let body = keyed(key, 2);
    let first = First::default();
    let client = &first;
    std::thread::scope(|scope| {
        let asked = scope.spawn(move || {
            let route = ("POST", "/write");
            let status = match exchange(&address, "acme", "key-acme", route, &body) {
                Ok((status, _)) => Some(status),
                Err(cause) => {
                    lgwks_std::trace::debug!(%cause, "held_then: the held request failed");
                    None
                }
            };
            client.tell(Heard::Answer(status));
            status.is_none()
        });
        assert_eq!(
            said_unless_answered(server, &first)?,
            format!("wrote {key}"),
            "the sync came first"
        );
        then(server)?;
        match asked.join() {
            Ok(failed) => Ok(failed),
            Err(payload) => {
                let why = match payload.downcast_ref::<&str>() {
                    Some(said) => *said,
                    None => "a panic with no text",
                };
                Err(format!("the client panicked: {why}").into())
            }
        }
    })
}

/// Catches a retry that writes twice, a write lost with its reply, and a replay that
/// answers what the retry asked rather than what landed.
#[test]
fn a_kill_between_the_write_and_its_reply_leaves_one_write_and_the_retry_finds_it() -> Outcome {
    let scratch = Scratch::new("command-journal-kill")?;
    let mut server = Server::start(scratch.path(), Some("held"))?;
    assert_eq!(server.write("acme", "before", 1)?, said(200, "written 1"));
    let failed = held_then(&mut server, "held", Server::kill)?;
    assert!(failed, "the client's request fails with the process");

    let mut server = Server::start(scratch.path(), None)?;
    assert_eq!(server.count("acme")?, said(200, "2"), "the write stands");
    assert_eq!(server.write("acme", "held", 99)?, said(200, "replayed 2"));
    assert_eq!(server.write("acme", "before", 99)?, said(200, "replayed 1"));
    assert_eq!(server.count("acme")?, said(200, "2"), "and no retry wrote");
    assert_eq!(server.write("acme", "after", 3)?, said(200, "written 3"));
    server.kill()?;

    let server = Server::start(scratch.path(), None)?;
    assert_eq!(server.count("acme")?, said(200, "3"));
    server.stop()
}

/// Catches a stall that keeps a stopping process alive, and a clean stop losing a sync.
#[test]
fn a_stop_during_a_held_write_exits_clean_and_keeps_the_write() -> Outcome {
    let scratch = Scratch::new("command-journal-stop")?;
    let mut server = Server::start(scratch.path(), Some("held"))?;
    // Whether the client heard back is the stop's to decide; the clean exit is the claim.
    let _client_failed: bool = held_then(&mut server, "held", Server::close)?;
    let server = Server::start(scratch.path(), None)?;
    assert_eq!(server.write("acme", "held", 9)?, said(200, "replayed 2"));
    server.stop()
}

/// Catches one identity reading or spending another's key, and a key that opens a
/// tenant it does not name.
#[test]
fn two_tenants_with_one_key_keep_their_own_write_and_refuse_each_others_bearer() -> Outcome {
    let scratch = Scratch::new("command-journal-tenants")?;
    let server = Server::start(scratch.path(), None)?;
    assert_eq!(server.write("acme", "same", 1)?, said(200, "written 1"));
    assert_eq!(server.write("globex", "same", 2)?, said(200, "written 2"));
    assert_eq!(server.write("acme", "same", 3)?, said(200, "replayed 1"));
    assert_eq!(server.write("globex", "same", 4)?, said(200, "replayed 2"));
    let route = ("POST", "/write");
    let body = keyed("stolen", 5);
    let stolen = exchange(&server.address, "acme", "key-globex", route, &body)?;
    assert_eq!(stolen.0, 401, "{stolen:?}");
    assert_eq!(server.count("acme")?, said(200, "1"));
    assert_eq!(server.count("globex")?, said(200, "1"));
    server.stop()
}

/// Catches two writers both landing one key, and a second answer that makes up a value:
/// each client names its own value, and exactly one lands.
#[test]
fn a_hundred_clients_with_one_key_land_one_write_and_all_hear_it() -> Outcome {
    let scratch = Scratch::new("command-journal-burst")?;
    let server = Server::start(scratch.path(), None)?;
    let answers = std::thread::scope(|scope| {
        let clients: Vec<_> = (0..100_u64)
            .map(|value| {
                let (server, body) = (&server, keyed("once", value));
                scope.spawn(
                    move || match server.request("acme", ("POST", "/write"), &body) {
                        Ok((status, text)) => (status, text),
                        Err(cause) => (0, cause.to_string()),
                    },
                )
            })
            .collect();
        let mut answers = Vec::new();
        for client in clients {
            answers.push(client.join());
        }
        answers
    });
    let mut bodies = Vec::new();
    for joined in answers {
        let (status, text) = joined.map_err(|_| "a client panicked")?;
        assert_eq!(status, 200, "{text}");
        bodies.push(text);
    }
    let (first, rest): (Vec<_>, Vec<_>) =
        bodies.iter().partition(|text| text.starts_with("written"));
    assert_eq!(first.len(), 1, "{bodies:?}");
    let value = first
        .first()
        .and_then(|text| text.strip_prefix("written "))
        .ok_or("a value")?;
    let replay = format!("replayed {value}");
    assert!(rest.iter().all(|text| **text == replay), "{bodies:?}");
    assert_eq!(server.count("acme")?, said(200, "1"));
    server.stop()
}

/// Catches an open-tenant table with no bound, and a corrupt journal taken for an empty
/// one or spreading to a neighbour.
#[test]
fn a_corrupt_journal_and_a_tenant_too_many_each_answer_503_and_touch_nobody_else() -> Outcome {
    let scratch = Scratch::new("command-journal-limits")?;
    let server = Server::start(scratch.path(), None)?;
    for at in 0..64 {
        let tenant = format!("t{at}");
        assert_eq!(server.write(&tenant, "k", 1)?, said(200, "written 1"));
    }
    let past = server.write("t64", "k", 1)?;
    assert_eq!(past.0, 503, "{past:?}");
    assert!(past.1.contains("retry"), "{past:?}");
    assert_eq!(server.write("t0", "k", 2)?, said(200, "replayed 1"));
    assert_eq!(server.write("t0", "j", 2)?, said(200, "written 2"));
    server.stop()?;

    let path = scratch.path().join("t0.journal");
    let mut bytes = std::fs::read(&path)?;
    let middle = bytes.len().div_euclid(2);
    *bytes.get_mut(middle).ok_or("a journal with bytes")? ^= 0xFF;
    std::fs::write(&path, bytes)?;
    let server = Server::start(scratch.path(), None)?;
    let corrupt = server.count("t0")?;
    assert_eq!(corrupt.0, 503, "{corrupt:?}");
    assert_eq!(server.count("t1")?, said(200, "1"));
    server.stop()
}

/// Catches a request that reaches the journal malformed, and a bad command line
/// starting anything.
#[test]
fn a_malformed_write_is_refused_before_the_journal_and_a_bare_run_says_how() -> Outcome {
    let scratch = Scratch::new("command-journal-bad")?;
    let server = Server::start(scratch.path(), None)?;
    let giant = "k".repeat(257);
    for body in [
        keyed("", 1),
        keyed(&giant, 1),
        String::from("{\"key\":\"k\"}"),
        String::from("{\"key\":"),
        String::new(),
    ] {
        let refused = server.request("acme", ("POST", "/write"), &body)?;
        assert_eq!(refused.0, 400, "{body:?}: {refused:?}");
    }
    assert_eq!(server.count("acme")?, said(200, "0"));
    assert_eq!(
        server.write("acme", &"k".repeat(256), 1)?,
        said(200, "written 1")
    );
    server.stop()?;
    let bare = Command::new(binary()).output()?;
    assert_eq!(bare.status.code(), Some(64));
    assert!(String::from_utf8_lossy(&bare.stderr).contains("usage: command-journal"));
    Ok(())
}
