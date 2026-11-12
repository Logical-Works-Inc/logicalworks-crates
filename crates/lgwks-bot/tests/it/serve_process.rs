//! The tenant HTTP edge for `serve` and `route` words (#782): restart and
//! process evidence.
//!
//! Each test names the requirement it proves in its doc comment. The exchange
//! shape, the shipped binary's path and the kill-and-wait child live once in
//! `crate::serve_edge`.

#![cfg(feature = "edge")]

use std::net::TcpListener;
use std::sync::Arc;

use lgwks_bot::rt::edge::EdgeStop;
use lgwks_bot::rt::serve::{RouteMethod, ServeOptions, TypedRoute};

use crate::scratch::Scratch;
use crate::serve_edge::{
    Counter, free_port, get, post, post_draft, ready_deadline, spawn_estate_serve, wait_ready,
};

/// Requirement: durable resume; a restart answers a reused key from records.
///
/// Proves INV-BOT-185: a kill and restart over the same store answers a reused
/// key from the run's records, and the body never runs twice.
#[test]
fn kill_restart_resumes_from_records() -> Result<(), String> {
    let store = Scratch::new("serve-resume").map_err(|error| error.to_string())?;
    let counter = Arc::new(Counter::default());
    let first_worker = Arc::clone(&counter);
    let first_route = TypedRoute::new(
        RouteMethod::Post,
        "/drafts",
        move |tenant: String, _body: Vec<u8>| {
            let first_worker = Arc::clone(&first_worker);
            async move { first_worker.fold(tenant.as_str()) }
        },
    );
    let first_options = ServeOptions::new(0, 1_000, store.path().to_path_buf())
        .map_err(|error| error.to_string())?;
    let first_listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("the first listener is refused: {error}"))?;
    let first_addr = first_listener
        .local_addr()
        .map_err(|error| format!("the first address is refused: {error}"))?;
    let first_stop = EdgeStop::new();
    let first_body = std::thread::scope(|scope| {
        let ending = first_stop.clone();
        let served = scope.spawn(move || {
            lgwks_bot::rt::serve::serve(
                &first_options,
                std::vec![first_route],
                first_listener,
                &ending,
            )
            .map_err(|error| error.to_string())
        });
        let probed =
            post(first_addr, "acme", "/drafts", b"{}", Some("order-7")).map(|answer| answer.2);
        first_stop.stop();
        match served.join() {
            Ok(joined) => {
                joined?;
                probed
            }
            Err(_) => Err(String::from("the first serve thread panicked")),
        }
    })?;
    let second_worker = Arc::clone(&counter);
    let second_route = TypedRoute::new(
        RouteMethod::Post,
        "/drafts",
        move |tenant: String, _body: Vec<u8>| {
            let second_worker = Arc::clone(&second_worker);
            async move { second_worker.fold(tenant.as_str()) }
        },
    );
    let second_options = ServeOptions::new(0, 1_000, store.path().to_path_buf())
        .map_err(|error| error.to_string())?;
    let second_listener = TcpListener::bind("127.0.0.1:0")
        .map_err(|error| format!("the second listener is refused: {error}"))?;
    let second_addr = second_listener
        .local_addr()
        .map_err(|error| format!("the second address is refused: {error}"))?;
    let second_stop = EdgeStop::new();
    let second_body = std::thread::scope(|scope| {
        let ending = second_stop.clone();
        let served = scope.spawn(move || {
            lgwks_bot::rt::serve::serve(
                &second_options,
                std::vec![second_route],
                second_listener,
                &ending,
            )
            .map_err(|error| error.to_string())
        });
        let probed =
            post(second_addr, "acme", "/drafts", b"{}", Some("order-7")).map(|answer| answer.2);
        second_stop.stop();
        match served.join() {
            Ok(joined) => {
                joined?;
                probed
            }
            Err(_) => Err(String::from("the second serve thread panicked")),
        }
    })?;
    if second_body != first_body {
        return Err(String::from("a restart did not return the recorded result"));
    }
    if counter.runs() != 1 {
        return Err(format!("a restart ran the body {} times", counter.runs()));
    }
    Ok(())
}

/// Requirement: the shipped binary; kill `-9` and restart over one store.
///
/// Proves INV-BOT-185: the shipped `estate serve` binary serves load, a real
/// `kill -9` loses no durable record, and a restart answers a reused key with
/// the first result.
#[test]
fn estate_serve_binary_survives_kill_9() -> Result<(), String> {
    let store = Scratch::new("serve-kill").map_err(|error| error.to_string())?;
    let first_port = free_port()?;
    let first_addr = std::net::SocketAddr::from(([127, 0, 0, 1], first_port));
    let mut first = spawn_estate_serve(store.path(), first_port, &[], std::process::Stdio::null())?;
    let body = b"{\"title\": \"T\", \"body\": \"B\"}";
    let (first_status, _, first_answer) = wait_ready(
        first_addr,
        "acme",
        "/drafts",
        body,
        Some("order-kill"),
        ready_deadline()?,
    )?;
    if first_status != 200 {
        return Err(format!("the first server answered {first_status}, not 200"));
    }
    let (load_status, _) = post_draft(first_addr, "other", "L", Some("order-load"))?;
    if load_status != 200 {
        return Err(format!(
            "the first server answered load {load_status}, not 200"
        ));
    }
    first.stop()?;
    let second_port = free_port()?;
    let second_addr = std::net::SocketAddr::from(([127, 0, 0, 1], second_port));
    let mut second =
        spawn_estate_serve(store.path(), second_port, &[], std::process::Stdio::null())?;
    let (second_status, _, second_answer) = wait_ready(
        second_addr,
        "acme",
        "/drafts",
        body,
        Some("order-kill"),
        ready_deadline()?,
    )?;
    if second_status != 200 {
        return Err(format!(
            "the restarted server answered {second_status}, not 200"
        ));
    }
    if second_answer != first_answer {
        return Err(String::from("a restart did not return the recorded result"));
    }
    let (third_status, _) = post_draft(second_addr, "other", "L2", Some("order-load-2"))?;
    if third_status != 200 {
        return Err(format!(
            "the restarted server answered {third_status}, not 200"
        ));
    }
    second.stop()?;
    Ok(())
}

/// Requirement: the shipped contract; the binary serves its own OpenAPI document.
///
/// Proves INV-BOT-190: `estate serve` pre-renders the described registry at
/// startup, refuses an undescribed registry before binding, and answers
/// `GET /t/{tenant}/openapi.json` with the 3.1 document naming the draft
/// operation.
#[test]
fn estate_serve_serves_its_openapi_contract() -> Result<(), String> {
    let store = Scratch::new("serve-contract").map_err(|error| error.to_string())?;
    let port = free_port()?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut server = spawn_estate_serve(store.path(), port, &[], std::process::Stdio::null())?;
    let body = b"{\"title\": \"T\", \"body\": \"B\"}";
    let (ready_status, _, _) = wait_ready(
        addr,
        "acme",
        "/drafts",
        body,
        Some("order-contract"),
        ready_deadline()?,
    )?;
    if ready_status != 200 {
        return Err(format!("the server answered {ready_status}, not 200"));
    }
    let (status, _, document) = get(addr, "acme", "/openapi.json")?;
    if status != 200 {
        return Err(format!("the contract answered {status}, not 200"));
    }
    for wanted in [
        "\"openapi\":\"3.1.0\"",
        "/drafts",
        "estate::serve::drafts",
        "/openapi.json",
        "estate::serve::openapi",
    ] {
        assert!(
            document.contains(wanted),
            "the served contract names {wanted}: {document:?}"
        );
    }
    server.stop()?;
    Ok(())
}

/// Requirement: the shipped binary stops gracefully on SIGTERM.
///
/// Proves INV-BOT-185: `estate serve` stops accepting on SIGTERM, drains within
/// `--drain-grace-ms`, exits 0 and prints what drained and what was cancelled.
#[cfg(unix)]
#[test]
fn estate_serve_drains_and_reports_on_sigterm() -> Result<(), String> {
    let store = Scratch::new("serve-term").map_err(|error| error.to_string())?;
    let port = free_port()?;
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let mut server = spawn_estate_serve(
        store.path(),
        port,
        &["--drain-grace-ms", "2000"],
        std::process::Stdio::piped(),
    )?;
    let body = b"{\"title\": \"T\", \"body\": \"B\"}";
    let (status, _, _) = wait_ready(
        addr,
        "acme",
        "/drafts",
        body,
        Some("order-term"),
        ready_deadline()?,
    )?;
    if status != 200 {
        return Err(format!("the server answered {status}, not 200"));
    }
    let (exit, stdout) = server.terminate()?;
    if !exit.success() {
        return Err(format!(
            "estate serve exited {exit} on SIGTERM; stdout: {stdout:?}"
        ));
    }
    let wanted = format!("stopped drafts on 127.0.0.1:{port}: drained=");
    let Some(report) = stdout
        .lines()
        .find(|line| line.starts_with(wanted.as_str()))
    else {
        return Err(format!("estate serve printed no stop report: {stdout:?}"));
    };
    if !report.ends_with("cancelled=0") {
        return Err(format!("an idle stop cancelled in-flight work: {report}"));
    }
    if std::net::TcpStream::connect(addr).is_ok() {
        return Err(String::from("the port still accepts after the stop"));
    }
    Ok(())
}

/// Requirement: the shipped binary drains a SIGTERM from the moment it says it serves.
///
/// Source: run 43219123584, where `estate serve` printed its ready line and
/// answered a request before its SIGTERM handler was installed on the watcher
/// thread, so the signal took the default action (exit by signal 15) instead
/// of draining. Proves INV-BOT-185 at the earliest instant a supervisor can
/// stop it: eight servers each get SIGTERM the moment their ready line is
/// read, and each exits 0 with its stop report and a closed port.
#[cfg(unix)]
#[test]
fn serve_ref_drains_a_sigterm_sent_at_its_ready_line() -> Result<(), String> {
    for round in 0..8_u8 {
        let store = Scratch::new("serve-term-ready").map_err(|error| error.to_string())?;
        let port = free_port()?;
        let mut server = spawn_estate_serve(
            store.path(),
            port,
            &["--drain-grace-ms", "2000"],
            std::process::Stdio::piped(),
        )?;
        let (exit, stdout) = server.terminate_at_ready()?;
        if !exit.success() {
            return Err(format!(
                "round {round}: estate serve exited {exit} on a SIGTERM at its ready line; stdout: {stdout:?}"
            ));
        }
        let wanted = format!("stopped drafts on 127.0.0.1:{port}: drained=");
        if !stdout
            .lines()
            .any(|line| line.starts_with(wanted.as_str()) && line.ends_with("cancelled=0"))
        {
            return Err(format!("round {round}: no stop report: {stdout:?}"));
        }
        let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        if std::net::TcpStream::connect(addr).is_ok() {
            return Err(format!(
                "round {round}: the port still accepts after the stop"
            ));
        }
    }
    Ok(())
}
