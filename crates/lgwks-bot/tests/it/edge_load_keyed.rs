//! The shipped `edge-load` binary: a probe reaches no tenant, and a key opens one.
//!
//! Source: the proofs a consumer named in #879 and #880: the binary answers `/healthz`,
//! no tenant appears in the feed's counts, and a wrong or missing key queues nothing.

use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};

use crate::sim::piped::{Outcome, located};

/// A consumer's build of `edge` alone failed on 7.1.1 and 8.0.0 with E0432, because `app`
/// imports `task` and `script` and only the `script` feature compiles them. No CI row builds
/// this feature alone, so the manifest's own line is what holds it.
#[test]
fn the_edge_feature_carries_the_modules_it_imports() {
    let edge = include_str!("../../Cargo.toml")
        .lines()
        .find(|line| line.starts_with("edge = ["));
    assert!(
        edge.is_some_and(|line| line.contains("\"script\"")),
        "`edge` must carry `script`: {edge:?}"
    );
}

/// The binary under test.
fn binary() -> PathBuf {
    located(
        ["NEXTEST_BIN_EXE_edge-load", "NEXTEST_BIN_EXE_edge_load"],
        env!("CARGO_BIN_EXE_edge-load"),
    )
}

/// A running server, and the address it announced.
fn start(mode: &str) -> Outcome<(Child, BufReader<ChildStdout>, SocketAddr)> {
    let mut server = Command::new(binary())
        .arg(mode)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let stdout = server.stdout.take().ok_or("no stdout")?;
    let mut lines = BufReader::new(stdout);
    let mut first = String::new();
    lines.read_line(&mut first)?;
    let address = first
        .trim()
        .strip_prefix("listening ")
        .ok_or("no address")?;
    let parsed = address.parse()?;
    Ok((server, lines, parsed))
}

/// The status and body of one request, given a key line or nothing.
fn hit(to: SocketAddr, request: &str, key: &str) -> Outcome<(u16, String)> {
    let mut wire = TcpStream::connect(to)?;
    let head = format!(
        "{request} HTTP/1.1\r\nhost: e\r\n{key}connection: close\r\ncontent-length: 0\r\n\r\n"
    );
    wire.write_all(head.as_bytes())?;
    let mut text = String::new();
    wire.read_to_string(&mut text)?;
    let (first, body) = text.split_once("\r\n\r\n").ok_or("no head")?;
    let status = first.split(' ').nth(1).ok_or("no status")?.parse()?;
    Ok((status, body.to_owned()))
}

/// Catches a probe that creates a tenant, or goes uncounted, on the world's edge.
#[test]
fn a_probe_on_the_worlds_edge_is_answered_counted_and_creates_no_tenant() -> Outcome {
    let (mut server, mut lines, to) = start("world")?;
    assert_eq!(hit(to, "GET /healthz", "")?, (200, "ok".to_owned()));
    assert_eq!(hit(to, "GET /healthz", "")?.0, 200);
    assert_eq!(hit(to, "POST /healthz", "")?.0, 400);
    drop(server.stdin.take());
    let mut record = String::new();
    lines.read_line(&mut record)?;
    server.wait()?;
    assert!(record.contains("tenants=0"), "{record}");
    assert!(record.contains("probes=2"), "{record}");
    assert!(
        TcpStream::connect(to).is_err(),
        "a stopped edge accepts nothing"
    );
    Ok(())
}

/// Catches a missing or foreign key reaching a tenant's state in the shipped binary.
#[test]
fn a_key_opens_only_its_own_tenant_and_a_refusal_counts_nothing() -> Outcome {
    let (mut server, _lines, to) = start("app-keyed")?;
    let (acme, globex) = (
        "authorization: Bearer key-acme\r\n",
        "x-api-key: key-globex\r\n",
    );
    let walked = (|| -> Outcome {
        assert_eq!(hit(to, "GET /healthz", "")?.0, 200, "a probe needs no key");
        assert_eq!(
            hit(to, "POST /t/acme/count", acme)?,
            (200, "acme:1".to_owned())
        );
        assert_eq!(hit(to, "POST /t/acme/count", "")?.0, 401);
        assert_eq!(hit(to, "POST /t/acme/count", globex)?.0, 401);
        assert_eq!(
            hit(to, "POST /t/globex/count", globex)?,
            (200, "globex:1".to_owned())
        );
        assert_eq!(
            hit(to, "GET /t/acme/count", acme)?,
            (200, "acme:1".to_owned())
        );
        Ok(())
    })();
    drop(server.stdin.take());
    server.wait()?;
    walked
}
