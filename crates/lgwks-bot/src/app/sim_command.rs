//! Seeded fools against a command on the app's host (#898, ADR 0025).
//!
//! Source: #898's six questions (how a request stays open, what answers a lost
//! answer, where a blocking command runs, how the principal arrives, what a kill
//! leaves, that a rebuild never re-runs) and RFC 9110's 503 and 504. A gate holds a
//! service where a sim needs it held, so nothing here sleeps or races a clock. The
//! oracle is a model or an invariant the fool breaks: every waiter is answered,
//! every permit comes back, no tenant sees another's asker. The fools that need a
//! full queue or a closing host are in `sim_command_faults`.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::sync_channel;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::seeded::Seeded;

use super::transition::{AppCell, AppMessage};
use super::{
    App, CommandLimits, Effect, EffectResult, Method, OnInput, Principal, Provider, ReplyTag,
};
use crate::cap::Cap;
use crate::cell::{CellHost, DynHost, MAX_MESSAGE_BYTES, ReplySlot};
use crate::ecs::Response;

/// A sim's outcome: setup failures propagate instead of panicking.
pub(super) type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// A host over [`Book`].
pub(super) type Host = Arc<CellHost<AppCell<Book>>>;

/// How long a sim allows, when no deadline is what it tries.
pub(super) const LONG: Duration = Duration::from_secs(600);

/// A service held open until the sim lets it go, counting who arrived.
#[derive(Default)]
pub(super) struct Gate {
    /// How many arrived, and whether the way is open.
    state: Mutex<(u64, bool)>,
    /// Rung on each arrival and when it opens.
    bell: Condvar,
}

impl Gate {
    /// Arrive, and wait for the way to open.
    pub(super) fn hold(&self) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        state.0 = state.0.saturating_add(1);
        self.bell.notify_all();
        while !state.1 {
            state = match self.bell.wait(state) {
                Ok(woke) => woke,
                Err(_) => return,
            };
        }
    }

    /// Wait until `count` have arrived.
    pub(super) fn arrived(&self, count: u64) -> Outcome {
        let mut state = self.state.lock().map_err(broke)?;
        while state.0 < count {
            state = self.bell.wait(state).map_err(broke)?;
        }
        Ok(())
    }

    /// Let everyone through, now and later.
    pub(super) fn open(&self) -> Outcome {
        self.state.lock().map_err(broke)?.1 = true;
        self.bell.notify_all();
        Ok(())
    }
}

/// A gate's lock poisoned by a panic, with what the lock said.
fn broke<T>(cause: std::sync::PoisonError<T>) -> String {
    format!("the gate broke: {cause}")
}

/// A provider that is one function.
pub(super) struct Service<F>(pub(super) F);

impl<F> Provider for Service<F>
where
    F: Fn(&str, Option<&Principal>, &[u8]) -> Option<Vec<u8>> + Send + Sync + 'static,
{
    fn dispatch(&self, _: &str, _: &Effect) -> Option<Vec<u8>> {
        None
    }

    fn command(&self, tenant: &str, principal: Option<&Principal>, body: &[u8]) -> Option<Vec<u8>> {
        (self.0)(tenant, principal, body)
    }
}

/// One tenant's fold of what its commands answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Book {
    /// Answers that carried bytes.
    landed: u64,
    /// Answers that said the service did not serve it.
    unavailable: u64,
    /// Answers that said the app holds no such cap.
    refused: u64,
}

/// A command for `text`, answered at the `done` handler.
pub(super) fn command_for(text: &str) -> Effect {
    Effect::Command {
        body: text.as_bytes().into(),
        reply_to: ReplyTag::new("done"),
    }
}

/// Fold an answer and answer the request with it.
pub(super) fn answered(book: &mut Book, input: &OnInput) -> Vec<Effect> {
    match *input.result() {
        EffectResult::Value(ref bytes) => {
            book.landed = book.landed.saturating_add(1);
            vec![Effect::reply(200, bytes.to_vec())]
        }
        EffectResult::Unavailable => {
            book.unavailable = book.unavailable.saturating_add(1);
            vec![Effect::reply(503, b"unavailable".to_vec())]
        }
        EffectResult::Refused(_) => {
            book.refused = book.refused.saturating_add(1);
            vec![Effect::reply(403, b"refused".to_vec())]
        }
    }
}

/// The app under simulation, with no cap yet: `POST /do` runs a command and holds
/// the request, and `GET /book` reads the fold.
pub(super) fn book_app(provider: impl Provider, limits: CommandLimits) -> App<Book> {
    App::new("sim-command")
        .state::<Book>()
        .provider(provider)
        .command_limits(limits)
        .route(Method::POST, "/do", |_: &mut Book, text: String| {
            vec![command_for(&text)]
        })
        .route(Method::GET, "/book", |book: &mut Book, (): ()| {
            let line = format!("{},{},{}", book.landed, book.unavailable, book.refused);
            vec![Effect::reply(200, line.into_bytes())]
        })
        .on(ReplyTag::new("done"), answered)
}

/// The app, granted commands and started with room for every level a sim drives.
pub(super) fn granted(provider: impl Provider, limits: CommandLimits) -> Outcome<Host> {
    Ok(book_app(provider, limits)
        .grant(Cap::new("app.command"))
        .start(4, 1 << 17)?)
}

/// Roomy bounds, for a sim about something else.
pub(super) fn roomy() -> CommandLimits {
    CommandLimits::new(4, 1_024, 1_024, LONG)
}

/// Queue one request, from `who`, and hand back what it will answer into.
pub(super) fn send(
    host: &Host,
    tenant: &str,
    (method, path): (Method, &str),
    body: &str,
    who: Option<&str>,
) -> Outcome<Arc<ReplySlot>> {
    let message = AppMessage::Route {
        tenant: tenant.into(),
        method,
        path: path.into(),
        body: format!("\"{body}\"").as_bytes().into(),
        principal: who.and_then(Principal::new),
    };
    let reply = Arc::new(ReplySlot::new());
    host.submit_wait(tenant, message, Arc::clone(&reply))?;
    Ok(reply)
}

/// `POST /do` for `tenant`, waiting for its answer.
pub(super) fn run_command(host: &Host, tenant: &str, text: &str) -> Outcome<Response> {
    let reply = send(host, tenant, (Method::POST, "/do"), text, None)?;
    Ok(reply.wait_blocking()?)
}

/// `GET /book` for `tenant`, as its line.
pub(super) fn book_of(host: &Host, tenant: &str) -> Outcome<String> {
    let message = AppMessage::Route {
        tenant: tenant.into(),
        method: Method::GET,
        path: "/book".into(),
        body: Box::default(),
        principal: None,
    };
    let reply = Arc::new(ReplySlot::new());
    host.submit_wait(tenant, message, Arc::clone(&reply))?;
    Ok(String::from_utf8(reply.wait_blocking()?.into_body())?)
}

/// A response with `status` and `body`.
pub(super) fn said(status: u16, body: &[u8]) -> Response {
    Response::new(status, body.to_vec())
}

/// Catches a command that takes the request when a reply already answered it, a
/// scan that sees a reply only in front of the command, and a late answer lost.
#[test]
fn sim_command_wrong_order_a_reply_beside_a_command_answers_at_once() -> Outcome {
    let gate = Arc::new(Gate::default());
    let held = Arc::clone(&gate);
    let service = Service(move |_: &str, _: Option<&Principal>, _: &[u8]| {
        held.hold();
        Some(b"late".to_vec())
    });
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/first", |_: &mut Book, text: String| {
            vec![Effect::reply(202, b"now".to_vec()), command_for(&text)]
        })
        .route(Method::POST, "/last", |_: &mut Book, text: String| {
            vec![command_for(&text), Effect::reply(202, b"now".to_vec())]
        })
        .start(2, 64)?;
    let tenant = "wrong-order";
    for path in ["/first", "/last"] {
        let reply = send(&host, tenant, (Method::POST, path), "x", None)?;
        assert_eq!(
            reply.wait_blocking()?,
            said(202, b"now"),
            "{path} answers while the service is still held"
        );
    }
    gate.arrived(2)?;
    gate.open()?;
    while book_of(&host, tenant)? != "2,0,0" {
        std::thread::yield_now();
    }
    host.close();
    Ok(())
}

/// Catches two commands both taking one request, and a second one's answer lost.
#[test]
fn sim_command_double_clicker_two_commands_hold_once_and_both_fold() -> Outcome {
    let seen = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&seen);
    let service = Service(move |_: &str, _: Option<&Principal>, body: &[u8]| {
        counted.fetch_add(1, Ordering::AcqRel);
        Some(body.to_vec())
    });
    let (sent, folded) = sync_channel::<Vec<u8>>(4);
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/twice", |_: &mut Book, _: String| {
            vec![
                Effect::Command {
                    body: b"one".as_slice().into(),
                    reply_to: ReplyTag::new("first"),
                },
                Effect::Command {
                    body: b"two".as_slice().into(),
                    reply_to: ReplyTag::new("second"),
                },
            ]
        })
        .on(
            ReplyTag::new("first"),
            |_: &mut Book, input: &OnInput| match *input.result() {
                EffectResult::Value(ref bytes) => vec![Effect::reply(200, bytes.to_vec())],
                EffectResult::Unavailable | EffectResult::Refused(_) => Vec::new(),
            },
        )
        .on(
            ReplyTag::new("second"),
            move |_: &mut Book, input: &OnInput| {
                // The check at start probes this handler with an empty answer.
                if let EffectResult::Value(ref bytes) = *input.result()
                    && !bytes.is_empty()
                {
                    drop(sent.try_send(bytes.to_vec()));
                }
                vec![Effect::reply(418, b"too late to answer".to_vec())]
            },
        )
        .start(2, 64)?;
    let reply = send(&host, "clicker", (Method::POST, "/twice"), "x", None)?;
    assert_eq!(
        reply.wait_blocking()?,
        said(200, b"one"),
        "the first command holds the request"
    );
    assert_eq!(folded.recv()?, b"two".to_vec(), "the second still folds");
    assert_eq!(seen.load(Ordering::Acquire), 2);
    host.close();
    Ok(())
}

/// Catches a held request left hanging when nothing answers it.
#[test]
fn sim_command_saboteur_a_held_request_nobody_answers_is_500_not_a_hang() -> Outcome {
    let service = Service(|_: &str, _: Option<&Principal>, _: &[u8]| Some(Vec::new()));
    let tagged = |tag: &'static str| {
        move |_: &mut Book, _: String| {
            vec![Effect::Command {
                body: Box::default(),
                reply_to: ReplyTag::new(tag),
            }]
        }
    };
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/nobody", tagged("no-such-handler"))
        .route(Method::POST, "/quiet", tagged("quiet"))
        .route(Method::POST, "/boom", tagged("boom"))
        .on(ReplyTag::new("quiet"), |_: &mut Book, _: &OnInput| {
            Vec::new()
        })
        .on(ReplyTag::new("boom"), |_: &mut Book, input: &OnInput| {
            assert!(
                !matches!(*input.result(), EffectResult::Value(ref bytes) if bytes.is_empty()),
                "the answer panics the handler"
            );
            Vec::new()
        })
        .start(2, 64)?;
    for path in ["/nobody", "/quiet"] {
        let reply = send(&host, "saboteur", (Method::POST, path), "x", None)?;
        assert_eq!(
            reply.wait_blocking()?,
            said(500, b"the cell returned no answer"),
            "{path}"
        );
    }
    let reply = send(&host, "saboteur", (Method::POST, "/boom"), "x", None)?;
    assert_eq!(
        reply.wait_blocking()?.status(),
        500,
        "a panicking `on` answers"
    );
    assert_eq!(host.counts().panicked, 1);
    host.close();
    Ok(())
}

/// Catches a service's failure reaching the client as a hang, and a dead pool.
#[test]
fn sim_command_liar_upstream_a_panic_a_none_and_a_giant_each_answer() -> Outcome {
    let service = Service(|_: &str, _: Option<&Principal>, body: &[u8]| {
        assert!(body != b"panic", "the service panics on this command");
        match body {
            b"none" => None,
            b"giant" => Some(vec![0; MAX_MESSAGE_BYTES.saturating_add(1)]),
            other => Some(other.to_vec()),
        }
    });
    let host = granted(service, CommandLimits::new(1, 8, 8, LONG))?;
    let tenant = "liar";
    assert_eq!(
        run_command(&host, tenant, "none")?,
        said(503, b"unavailable")
    );
    assert_eq!(
        run_command(&host, tenant, "panic")?,
        said(503, b"unavailable")
    );
    let giant = run_command(&host, tenant, "giant")?;
    assert_eq!(giant.status(), 502, "an answer past the ceiling is told so");
    assert_eq!(
        run_command(&host, tenant, "fine")?,
        said(200, b"fine"),
        "the one thread survived all three"
    );
    assert_eq!(book_of(&host, tenant)?, "1,2,0", "the giant never folded");
    host.close();
    Ok(())
}

/// Catches a cut command, which would be a wrong act, and an off-by-one at its bound.
#[test]
fn sim_command_giant_and_empty_a_command_is_whole_or_not_run() -> Outcome {
    let lengths = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&lengths);
    let service = Service(move |_: &str, _: Option<&Principal>, body: &[u8]| {
        if let Ok(mut held) = seen.lock() {
            held.push(body.len());
        }
        Some(Vec::new())
    });
    let host = book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/sized", |_: &mut Book, size: String| {
            let mut body = Vec::new();
            if let Ok(bytes) = size.parse::<usize>() {
                body.resize(bytes, b'x');
            }
            vec![Effect::Command {
                body: body.into_boxed_slice(),
                reply_to: ReplyTag::new("done"),
            }]
        })
        .start(2, 64)?;
    for (size, status) in [
        (1, 200),
        (MAX_MESSAGE_BYTES, 200),
        (MAX_MESSAGE_BYTES.saturating_add(1), 503),
        (MAX_MESSAGE_BYTES.saturating_mul(10), 503),
    ] {
        let path = (Method::POST, "/sized");
        let reply = send(&host, "giant", path, &size.to_string(), None)?;
        assert_eq!(reply.wait_blocking()?.status(), status, "a body of {size}");
    }
    let reply = send(&host, "giant", (Method::POST, "/sized"), "", None)?;
    assert_eq!(
        reply.wait_blocking()?.status(),
        200,
        "an empty command runs"
    );
    let held = lengths.lock().map_err(|_| "the list broke")?.clone();
    assert_eq!(
        held,
        vec![1, MAX_MESSAGE_BYTES, 0],
        "what ran was whole, and nothing past the ceiling ran"
    );
    host.close();
    Ok(())
}

/// Catches a queued command run long after anyone could want it.
#[test]
fn sim_command_clock_fool_a_command_past_its_deadline_is_not_run() -> Outcome {
    let seen = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&seen);
    let service = Service(move |_: &str, _: Option<&Principal>, _: &[u8]| {
        counted.fetch_add(1, Ordering::AcqRel);
        Some(Vec::new())
    });
    let host = granted(service, CommandLimits::new(2, 8, 8, Duration::ZERO))?;
    for round in 0..16 {
        let answer = run_command(&host, "clock", &round.to_string())?;
        assert_eq!(answer, said(503, b"unavailable"), "a deadline already past");
    }
    assert_eq!(
        seen.load(Ordering::Acquire),
        0,
        "and the service never saw one"
    );
    assert_eq!(book_of(&host, "clock")?, "0,16,0");
    host.close();
    Ok(())
}

/// Catches a command reaching the service with no grant, and a default provider
/// that hangs or invents an answer.
#[test]
fn sim_command_fat_finger_no_grant_refuses_and_no_service_is_unavailable() -> Outcome {
    let seen = Arc::new(AtomicU64::new(0));
    let counted = Arc::clone(&seen);
    let service = Service(move |_: &str, _: Option<&Principal>, _: &[u8]| {
        counted.fetch_add(1, Ordering::AcqRel);
        Some(Vec::new())
    });
    let refused = App::new("no-grant")
        .state::<Book>()
        .provider(service)
        .route(Method::POST, "/hidden", |_: &mut Book, text: String| {
            // Hidden from every probe the check tries, as the runner has to assume.
            if text == "hidden" {
                vec![command_for(&text)]
            } else {
                vec![Effect::reply(200, Vec::new())]
            }
        })
        .on(ReplyTag::new("done"), answered)
        .start(2, 64)?;
    let reply = send(&refused, "fat", (Method::POST, "/hidden"), "hidden", None)?;
    assert_eq!(reply.wait_blocking()?, said(403, b"refused"));
    assert_eq!(seen.load(Ordering::Acquire), 0, "the service never saw it");
    refused.close();

    let shown = book_app(
        Service(|_: &str, _: Option<&Principal>, _: &[u8]| None),
        roomy(),
    );
    assert!(
        shown.check().is_err(),
        "a handler that shows its command needs the grant"
    );

    let bare = App::new("no-service")
        .state::<Book>()
        .grant(Cap::new("app.command"))
        .route(Method::POST, "/do", |_: &mut Book, text: String| {
            vec![command_for(&text)]
        })
        .on(ReplyTag::new("done"), answered)
        .start(2, 64)?;
    let reply = send_bare(&bare)?;
    assert_eq!(
        reply.wait_blocking()?,
        said(503, b"unavailable"),
        "a provider with no command method answers that it serves none"
    );
    bare.close();
    Ok(())
}

/// One request to the app with no service.
fn send_bare(host: &Host) -> Outcome<Arc<ReplySlot>> {
    send(host, "fat", (Method::POST, "/do"), "x", None)
}

/// What a service answers for `body`: a function of everything it is handed, and
/// nothing one time in five.
fn modelled(tenant: &str, who: Option<&str>, body: &[u8]) -> Option<Vec<u8>> {
    let line = format!("{tenant}|{who:?}|{}", String::from_utf8_lossy(body));
    let mixed = lgwks_std::hash::blake3(line.as_bytes());
    let [first, ..] = *mixed.as_bytes();
    (!first.is_multiple_of(5)).then(|| line.into_bytes())
}

/// One seed's walk: every answer against a model of the tenants' books, hashed.
fn monkey(seed: u64) -> Outcome<Digest> {
    let mut stream = Seeded::from_seed(seed);
    let service = Service(|tenant: &str, who: Option<&Principal>, body: &[u8]| {
        modelled(tenant, who.map(Principal::as_str), body)
    });
    let host = granted(service, CommandLimits::new(4, 64, 16, LONG))?;
    let tenants = stream.below(16)?.saturating_add(2);
    let operations = stream.below(200)?.saturating_add(200);
    let mut model: BTreeMap<String, Book> = BTreeMap::new();
    let mut hasher = Hasher::new();
    for _ in 0..operations {
        let tenant = format!("t{}", stream.below(tenants)?);
        let drawn = usize::try_from(stream.below(3)?)?;
        let who = ["alice", "bob"].get(drawn).copied();
        let text = stream.below(64)?.to_string();
        let reply = send(&host, &tenant, (Method::POST, "/do"), &text, who)?;
        let answer = reply.wait_blocking()?;
        let book = model.entry(tenant.clone()).or_default();
        let expected = match modelled(&tenant, who, text.as_bytes()) {
            Some(bytes) => {
                book.landed = book.landed.saturating_add(1);
                Response::new(200, bytes)
            }
            None => {
                book.unavailable = book.unavailable.saturating_add(1);
                said(503, b"unavailable")
            }
        };
        assert_eq!(answer, expected, "seed {seed}, tenant {tenant}");
        hasher.update(&answer.status().to_be_bytes());
        hasher.update(&answer.into_body());
    }
    for (tenant, book) in &model {
        let line = format!("{},{},{}", book.landed, book.unavailable, book.refused);
        assert_eq!(
            book_of(&host, tenant)?,
            line,
            "seed {seed}, tenant {tenant}"
        );
    }
    host.close();
    Ok(hasher.finalize())
}

/// Catches an answer that depends on anything but the request, and a lost fold.
#[test]
fn sim_command_monkey_every_answer_matches_the_model_and_a_seed_replays() -> Outcome {
    for seed in 0..8 {
        assert_eq!(
            monkey(seed)?,
            monkey(seed)?,
            "seed {seed} replayed differently"
        );
    }
    assert_ne!(monkey(1)?, monkey(2)?, "two seeds drew one walk");
    Ok(())
}
