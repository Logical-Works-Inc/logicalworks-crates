//! The tenant HTTP edge for `serve` and `route` words (#782), over loopback.
//!
//! Each test names the requirement it proves in its doc comment. The exchange
//! shape, the served scope and the counting, parking and decoding tools live
//! once in `crate::serve_edge`.

#![cfg(feature = "edge")]

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

use lgwks_bot::rt::edge::{EdgeStop, MAX_BODY_BYTES};
use lgwks_bot::rt::serve::{RouteMethod, ServeOptions, TypedRoute};

use crate::scratch::Scratch;
use crate::serve_edge::{
    Gate, await_entered, counted, counting_route, decode_draft, loopback, parking_route,
    parse_count, post, post_timeout, tallying_route, with_serve,
};

/// Requirement: tenant isolation; hammering one route never crosses tenants.
///
/// Proves INV-BOT-185: the tenant comes from the path and is part of every
/// store key, so two tenants on the same resource ids keep their own counts.
#[test]
fn two_tenants_hammering_one_route_stay_isolated() -> Result<(), String> {
    let (store, counter) = counted("serve-isolated")?;
    let route = counting_route(&counter);
    with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        std::thread::scope(|scope| {
            let mut clients = Vec::new();
            for tenant in ["acme", "other"] {
                for thread in 0..2 {
                    clients.push(scope.spawn(move || {
                        let mut seen = Vec::new();
                        for index in 0..25 {
                            let key = format!("{tenant}-{thread}-{index}");
                            let (status, _, body) =
                                post(addr, tenant, "/drafts", b"{}", Some(key.as_str()))?;
                            if status != 200 {
                                return Err(format!("{tenant} answered {status}"));
                            }
                            seen.push(parse_count(body.as_str())?);
                        }
                        Ok((tenant, seen))
                    }));
                }
            }
            let mut acme: Vec<u64> = Vec::new();
            let mut other: Vec<u64> = Vec::new();
            for client in clients {
                match client.join() {
                    Ok(joined) => {
                        let (tenant, mut seen) = joined?;
                        if seen.len() != 25 {
                            return Err(format!("a client saw {} answers, not 25", seen.len()));
                        }
                        if tenant == "acme" {
                            acme.append(&mut seen);
                        } else {
                            other.append(&mut seen);
                        }
                    }
                    Err(_) => {
                        return Err(String::from("a client thread panicked"));
                    }
                }
            }
            acme.sort();
            other.sort();
            let mut want: Vec<u64> = Vec::new();
            for count in 1..=50 {
                want.push(count);
            }
            if acme != want {
                return Err(String::from(
                    "acme did not see exactly its own fifty counts",
                ));
            }
            if other != want {
                return Err(String::from(
                    "other did not see exactly its own fifty counts",
                ));
            }
            Ok(())
        })
    })?;
    if counter.runs() != 100 {
        return Err(format!(
            "two tenants ran {} bodies, not 100",
            counter.runs()
        ));
    }
    Ok(())
}

/// Requirement: idempotency; a retry returns the first result and runs once.
///
/// Proves INV-BOT-185: each request that starts a run gets a key and a retry
/// with the same key returns the first result from the run's records.
#[test]
fn an_idempotent_retry_returns_the_first_result() -> Result<(), String> {
    let (store, counter) = counted("serve-once")?;
    let route = counting_route(&counter);
    let body = b"{}";
    let ((first, second), _report) = with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        let (first_status, _, first_body) = post(addr, "acme", "/drafts", body, Some("order-1"))?;
        let (second_status, _, second_body) = post(addr, "acme", "/drafts", body, Some("order-1"))?;
        Ok(((first_status, first_body), (second_status, second_body)))
    })?;
    if first != second {
        return Err(String::from("a retry did not return the first result"));
    }
    if first.0 != 200 {
        return Err(format!("a retry answered {}", first.0));
    }
    if counter.runs() != 1 {
        return Err(format!(
            "a retry ran the body {} times, not once",
            counter.runs()
        ));
    }
    Ok(())
}

/// Requirement: derived keys; requests without a header derive one key.
///
/// Proves INV-BOT-185: a request with no `Idempotency-Key` derives its key
/// from tenant, route and body digest, so an identical repeat replays.
#[test]
fn requests_without_a_header_derive_one_key() -> Result<(), String> {
    let (store, counter) = counted("serve-derived")?;
    let route = counting_route(&counter);
    let body = b"{}";
    let ((first, second), _report) = with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        let (_, _, first_body) = post(addr, "acme", "/drafts", body, None)?;
        let (_, _, second_body) = post(addr, "acme", "/drafts", body, None)?;
        Ok((first_body, second_body))
    })?;
    if first != second {
        return Err(String::from(
            "a derived repeat did not return the first result",
        ));
    }
    if counter.runs() != 1 {
        return Err(format!(
            "a derived repeat ran the body {} times",
            counter.runs()
        ));
    }
    Ok(())
}

/// Requirement: conflicts; a reused key with a different body is refused.
///
/// Proves INV-BOT-185: the same key with a different input is a typed `422`
/// naming the conflict, and the first result still replays.
#[test]
fn a_reused_key_with_a_different_body_is_a_422_conflict() -> Result<(), String> {
    let (store, counter) = counted("serve-clash")?;
    let route = counting_route(&counter);
    let ((first, clashed, replayed), _report) =
        with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
            let (_, _, first) = post(addr, "acme", "/drafts", b"{\"n\":1}", Some("order-9"))?;
            let (clash_status, _, clash_body) =
                post(addr, "acme", "/drafts", b"{\"n\":2}", Some("order-9"))?;
            let (_, _, replayed) = post(addr, "acme", "/drafts", b"{\"n\":1}", Some("order-9"))?;
            Ok((first, (clash_status, clash_body), replayed))
        })?;
    if clashed.0 != 422 {
        return Err(format!(
            "a conflicting reuse answered {}, not 422",
            clashed.0
        ));
    }
    if !clashed.1.contains("already bound") {
        return Err(format!("a conflict names no binding: {:?}", clashed.1));
    }
    if replayed != first {
        return Err(String::from(
            "the first input no longer replays after a conflict",
        ));
    }
    if counter.runs() != 1 {
        return Err(format!("a conflict ran the body {} times", counter.runs()));
    }
    Ok(())
}

/// Requirement: typed bodies; malformed input names the field.
///
/// Proves INV-BOT-185: a body that does not decode is answered `400` naming
/// the field, before the handler's work lands.
#[test]
fn a_malformed_body_is_400_naming_the_field() -> Result<(), String> {
    let store = Scratch::new("serve-typed").map_err(|error| error.to_string())?;
    let route = TypedRoute::new(
        RouteMethod::Post,
        "/drafts",
        move |_tenant: String, body: Vec<u8>| async move {
            decode_draft(body.as_slice()).map(|text| text.into_bytes())
        },
    );
    let ((missing_status, missing_body, garbage_status), _report) =
        with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
            let (missing_status, _, missing_body) =
                post(addr, "acme", "/drafts", b"{\"body\": 1}", None)?;
            let (garbage_status, _, _) = post(addr, "acme", "/drafts", b"nope", None)?;
            Ok((missing_status, missing_body, garbage_status))
        })?;
    if missing_status != 400 {
        return Err(format!(
            "a malformed body answered {missing_status}, not 400"
        ));
    }
    if !missing_body.contains("title") {
        return Err(format!("a malformed body names no field: {missing_body:?}"));
    }
    if garbage_status != 400 {
        return Err(format!(
            "a non-JSON body answered {garbage_status}, not 400"
        ));
    }
    Ok(())
}

/// Requirement: bounded bodies; an oversize body is refused while streaming.
///
/// Proves INV-BOT-185: the size bound is checked before allocation and a body
/// over the bound is a `413`, refused while it streams.
#[test]
fn an_oversized_body_is_413_while_streaming() -> Result<(), String> {
    let store = Scratch::new("serve-bound").map_err(|error| error.to_string())?;
    let runs = Arc::new(AtomicUsize::new(0));
    let route = tallying_route(&runs, Ok);
    let wide = MAX_BODY_BYTES.saturating_add(1);
    let mut body = Vec::with_capacity(wide);
    body.resize(wide, b'a');
    let (status, _report) = with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        let (status, _, _) = post(addr, "acme", "/drafts", body.as_slice(), None)?;
        Ok(status)
    })?;
    if status != 413 {
        return Err(format!("an oversize body answered {status}, not 413"));
    }
    if runs.load(Ordering::SeqCst) != 0 {
        return Err(String::from("an oversize body reached the handler"));
    }
    Ok(())
}

/// Requirement: backpressure; past the ceiling the edge sheds with `Retry-After`.
///
/// Proves INV-BOT-185: over the in-flight ceiling the answer is `503` with
/// `Retry-After`, and there is never an unbounded queue.
#[test]
fn over_the_ceiling_is_503_with_retry_after() -> Result<(), String> {
    let store = Scratch::new("serve-shed").map_err(|error| error.to_string())?;
    let gate = Arc::new(Gate::default());
    let entered = Arc::new(AtomicBool::new(false));
    let route = parking_route("/drafts", &gate, &entered);
    let ((shed_status, shed_headers, first_status), report) =
        with_serve(std::vec![route], &store, 1, |addr, _stop| {
            std::thread::scope(|scope| {
                let held = scope.spawn(move || post(addr, "acme", "/drafts", b"park", None));
                await_entered(&entered)?;
                let (shed_status, shed_headers, _) = post(addr, "acme", "/drafts", b"park", None)?;
                gate.release()?;
                match held.join() {
                    Ok(joined) => {
                        let (first_status, _, _) = joined?;
                        Ok((shed_status, shed_headers, first_status))
                    }
                    Err(_) => Err(String::from("the parked client panicked")),
                }
            })
        })?;
    if shed_status != 503 {
        return Err(format!(
            "a request past the ceiling answered {shed_status}, not 503"
        ));
    }
    if !shed_headers.to_lowercase().contains("retry-after:") {
        return Err(String::from("a shed request names no retry delay"));
    }
    if first_status != 200 {
        return Err(format!(
            "the parked request answered {first_status}, not 200"
        ));
    }
    if report.drained() != 1 {
        return Err(format!(
            "the shed serve drained {} runs, not one",
            report.drained()
        ));
    }
    Ok(())
}

/// Requirement: exactly-once under concurrency; a claimed key runs once.
///
/// Proves INV-BOT-185: two concurrent requests with one key admit one run;
/// the second observes the claim instead of running the body again.
#[test]
fn a_claimed_key_runs_once_under_concurrency() -> Result<(), String> {
    let store = Scratch::new("serve-claim").map_err(|error| error.to_string())?;
    let gate = Arc::new(Gate::default());
    let parked = Arc::clone(&gate);
    let runs = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&runs);
    let entered_barrier = Arc::new(Barrier::new(2));
    let arrived = Arc::clone(&entered_barrier);
    let route = TypedRoute::new(
        RouteMethod::Post,
        "/drafts",
        move |_tenant: String, _body: Vec<u8>| {
            let parked = Arc::clone(&parked);
            let counted = Arc::clone(&counted);
            let arrived = Arc::clone(&arrived);
            async move {
                counted.fetch_add(1, Ordering::SeqCst);
                let _arrived = arrived.wait();
                parked.pass().map_err(|message| (500u16, message))
            }
        },
    );
    let body = b"{\"n\":1}";
    let ((first_status, second_status), _report) =
        with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
            std::thread::scope(|scope| {
                let racing =
                    scope.spawn(move || post(addr, "acme", "/drafts", body, Some("race-1")));
                entered_barrier.wait();
                let (second_status, _, _) = post(addr, "acme", "/drafts", body, Some("race-1"))?;
                gate.release()?;
                match racing.join() {
                    Ok(joined) => {
                        let (first_status, _, _) = joined?;
                        Ok((first_status, second_status))
                    }
                    Err(_) => Err(String::from("the racing client panicked")),
                }
            })
        })?;
    if first_status != 200 {
        return Err(format!(
            "the first of two racers answered {first_status}, not 200"
        ));
    }
    if second_status != 503 {
        return Err(format!(
            "the second of two racers answered {second_status}, not 503"
        ));
    }
    if runs.load(Ordering::SeqCst) != 1 {
        return Err(format!(
            "two racers ran the body {} times",
            runs.load(Ordering::SeqCst)
        ));
    }
    Ok(())
}

/// Requirement: no nested runtime; concurrent requests drain through blocking serve.
///
/// Proves INV-BOT-185 and guards the serve_load stall: the request-key path
/// runs on a blocking-pool thread on that thread's own private runtime, never
/// as a second `block_on` on the driver `serve` already holds.
#[test]
fn a_hundred_concurrent_requests_drain_through_blocking_serve() -> Result<(), String> {
    let (store, counter) = counted("serve-flood")?;
    let route = counting_route(&counter);
    let bound = Some(Duration::from_secs(20));
    let ((ok, total), report) = with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        std::thread::scope(|scope| {
            let mut clients = Vec::new();
            for client in 0..10 {
                clients.push(scope.spawn(move || {
                    let mut ok = 0_usize;
                    for index in 0..10 {
                        let tenant = if client < 5 { "acme" } else { "other" };
                        let key = format!("flood-{client}-{index}");
                        let (status, _, _) = post_timeout(
                            addr,
                            tenant,
                            "/drafts",
                            b"{}",
                            Some(key.as_str()),
                            bound,
                        )?;
                        if status != 200 {
                            return Err(format!("a flood request answered {status}"));
                        }
                        ok = ok.saturating_add(1);
                    }
                    Ok(ok)
                }));
            }
            let mut ok = 0_usize;
            for client in clients {
                match client.join() {
                    Ok(joined) => {
                        ok = ok.saturating_add(joined?);
                    }
                    Err(_) => {
                        return Err(String::from("a flood client panicked"));
                    }
                }
            }
            Ok((ok, 100_usize))
        })
    })?;
    if ok != total {
        return Err(format!("the flood answered {ok} of {total} requests"));
    }
    if counter.runs() != 100 {
        return Err(format!(
            "the flood ran the body {} times, not 100",
            counter.runs()
        ));
    }
    if report.drained() != 100 {
        return Err(format!(
            "the flood drained {} runs, not 100",
            report.drained()
        ));
    }
    Ok(())
}

/// Requirement: graceful stop; the report counts drained and cancelled.
///
/// Proves INV-BOT-185: stop accepting, drain within the declared deadline,
/// cancel the rest, and report what drained and what was cancelled.
#[test]
fn graceful_stop_drains_then_cancels_and_reports() -> Result<(), String> {
    let store = Scratch::new("serve-grace").map_err(|error| error.to_string())?;
    let gate = Arc::new(Gate::default());
    let entered = Arc::new(AtomicBool::new(false));
    let quick = TypedRoute::new(
        RouteMethod::Post,
        "/quick",
        move |_tenant: String, _body: Vec<u8>| async move { Ok(b"quick".to_vec()) },
    );
    let park = parking_route("/park", &gate, &entered);
    let options = ServeOptions::new(0, 1_000, store.path().to_path_buf())
        .map_err(|error| error.to_string())
        .map(|options| options.with_drain_grace(Duration::from_millis(200)))?;
    let (listener, addr) = loopback()?;
    let stop = EdgeStop::new();
    let (quick_status, report) = std::thread::scope(|scope| {
        let ending = stop.clone();
        let served = scope.spawn(move || {
            lgwks_bot::rt::serve::serve(&options, std::vec![quick, park], listener, &ending)
                .map_err(|error| error.to_string())
        });
        let held = scope.spawn(move || post(addr, "acme", "/park", b"park", None));
        await_entered(&entered)?;
        let (quick_status, _, _) = post(addr, "acme", "/quick", b"quick", None)?;
        stop.stop();
        let report = match served.join() {
            Ok(joined) => joined?,
            Err(_) => {
                return Err(String::from("the serve thread panicked"));
            }
        };
        gate.release()?;
        let held_done = held.join().is_ok();
        let _ = held_done;
        Ok((quick_status, report))
    })?;
    if quick_status != 200 {
        return Err(format!(
            "the quick request answered {quick_status}, not 200"
        ));
    }
    if report.drained() != 1 {
        return Err(format!(
            "the graceful stop drained {} runs, not one",
            report.drained()
        ));
    }
    if report.cancelled() != 1 {
        return Err(format!(
            "the graceful stop cancelled {} runs, not one",
            report.cancelled()
        ));
    }
    Ok(())
}

/// Requirement: hostile bodies never panic and never allocate past the bound.
///
/// Proves INV-BOT-185: fuzz-shaped bodies are answered `400` or `413`, the
/// handler runs only for bodies within the bound, and nothing panics.
#[test]
fn hostile_bodies_never_panic_nor_allocate_past_the_bound() -> Result<(), String> {
    let store = Scratch::new("serve-hostile").map_err(|error| error.to_string())?;
    let runs = Arc::new(AtomicUsize::new(0));
    let route = tallying_route(&runs, |body| {
        decode_draft(body.as_slice()).map(String::into_bytes)
    });
    let mut wide = b"title:".to_vec();
    wide.resize(MAX_BODY_BYTES.saturating_add(1), b'x');
    let mut nested = b"[".to_vec();
    nested.resize(8_192, b'[');
    let shapes: Vec<Vec<u8>> = std::vec![
        Vec::new(),
        b"{".to_vec(),
        b"not json".to_vec(),
        std::vec![0xff, 0xfe, 0xfd],
        b"null".to_vec(),
        b"{\"title\": 1}".to_vec(),
        wide,
        nested,
    ];
    let answered = with_serve(std::vec![route], &store, 1_000, |addr, _stop| {
        let mut statuses = Vec::with_capacity(shapes.len());
        for (index, shape) in shapes.iter().enumerate() {
            let key = format!("hostile-{index}");
            let (status, _, _) = post(
                addr,
                "acme",
                "/drafts",
                shape.as_slice(),
                Some(key.as_str()),
            )?;
            statuses.push(status);
        }
        Ok(statuses)
    })?;
    let (statuses, _report) = answered;
    if statuses.len() != 8 {
        return Err(format!(
            "hostile shapes answered {} times, not 8",
            statuses.len()
        ));
    }
    for status in statuses.iter() {
        if *status != 400 && *status != 413 {
            return Err(format!("a hostile body answered {status}, not 400 or 413"));
        }
    }
    if runs.load(Ordering::SeqCst) != 7 {
        return Err(format!(
            "hostile bodies ran the handler {} times, not 7",
            runs.load(Ordering::SeqCst)
        ));
    }
    Ok(())
}
