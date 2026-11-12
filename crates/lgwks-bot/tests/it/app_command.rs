//! A command held open over a real socket (#898).
//!
//! Source: #898's "the request stays open with bounds and typed answers" and RFC
//! 9110's 504. A gate holds the service, so the deadline passing is the only thing a
//! client can be waiting on and nothing races a clock.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use lgwks_bot::Cap;
use lgwks_bot::app::{
    App, CommandLimits, Effect, EffectResult, Method, OnInput, Principal, Provider, ReplyTag,
};

use crate::app::{JourneyFailed, serve_app};

/// Holds a service until the test lets it go, and says when one arrived.
#[derive(Default)]
struct Gate {
    /// How many arrived, and whether the way is open.
    state: Mutex<(u64, bool)>,
    /// Rung on each arrival and when it opens.
    bell: Condvar,
}

impl Gate {
    /// Arrive and wait for the way to open; `None` if the gate broke.
    fn hold(&self) -> Option<()> {
        let mut state = self.state.lock().ok()?;
        state.0 = state.0.saturating_add(1);
        self.bell.notify_all();
        while !state.1 {
            state = self.bell.wait(state).ok()?;
        }
        Some(())
    }

    /// Wait until one has arrived.
    fn arrived(&self) -> Result<(), JourneyFailed> {
        let mut state = self.state.lock().map_err(broke)?;
        while state.0 == 0 {
            state = self.bell.wait(state).map_err(broke)?;
        }
        Ok(())
    }

    /// Let everyone through.
    fn open(&self) -> Result<(), JourneyFailed> {
        self.state.lock().map_err(broke)?.1 = true;
        self.bell.notify_all();
        Ok(())
    }
}

/// A gate's lock poisoned by a panic, said before the journey fails on it.
fn broke<T>(cause: std::sync::PoisonError<T>) -> JourneyFailed {
    lgwks_std::trace::warn!(error = %cause, "app_command: the gate broke");
    JourneyFailed::Panicked
}

/// A service that answers once the gate lets it, and counts what it served.
struct Held {
    /// Waited on before each answer.
    gate: Arc<Gate>,
    /// Commands whose answer it returned.
    served: Arc<AtomicU64>,
}

impl Provider for Held {
    fn dispatch(&self, _: &str, _: &Effect) -> Option<Vec<u8>> {
        None
    }

    fn command(&self, tenant: &str, who: Option<&Principal>, body: &[u8]) -> Option<Vec<u8>> {
        self.gate.hold()?;
        self.served.fetch_add(1, Ordering::AcqRel);
        let name = who.map(Principal::as_str);
        let text = String::from_utf8_lossy(body);
        Some(format!("{tenant}:{name:?}:{text}").into_bytes())
    }
}

/// An app with one held command, answered and folded at one handler.
fn held_app(provider: Held, deadline: Duration) -> App<u64> {
    App::new("held-command")
        .state::<u64>()
        .grant(Cap::new("app.command"))
        .provider(provider)
        .command_limits(CommandLimits::new(2, 8, 8, deadline))
        .route(Method::POST, "/do", |_: &mut u64, text: String| {
            vec![Effect::Command {
                body: text.into_bytes().into_boxed_slice(),
                reply_to: ReplyTag::new("done"),
            }]
        })
        .route(Method::GET, "/folded", |count: &mut u64, (): ()| {
            vec![Effect::reply(200, count.to_string().into_bytes())]
        })
        .on(
            ReplyTag::new("done"),
            |count: &mut u64, input: &OnInput| match *input.result() {
                EffectResult::Value(ref bytes) => {
                    *count = count.saturating_add(1);
                    vec![Effect::reply(200, bytes.to_vec())]
                }
                _ => vec![Effect::reply(503, b"unavailable".to_vec())],
            },
        )
}

/// A held service, its gate and its count.
fn rig() -> (Held, Arc<Gate>, Arc<AtomicU64>) {
    let (gate, served) = (Arc::new(Gate::default()), Arc::new(AtomicU64::new(0)));
    let held = Held {
        gate: Arc::clone(&gate),
        served: Arc::clone(&served),
    };
    (held, gate, served)
}

/// Catches an answer the `on` handler wrote never reaching the socket, and a host
/// whose worker is the thread a service blocks: a second tenant reads while it waits.
#[test]
fn a_held_request_answers_from_the_on_handler_while_the_worker_serves_others()
-> Result<(), Box<dyn std::error::Error>> {
    let (provider, gate, served) = rig();
    let long = Duration::from_secs(600);
    serve_app(held_app(provider, long), |client| {
        std::thread::scope(|scope| -> Result<(), JourneyFailed> {
            let asked = scope.spawn(|| client.exchange("POST", "/t/acme/do", b"\"x\""));
            gate.arrived()?;
            client.answers("GET", "/t/acme/folded", b"", 200, "0")?;
            client.answers("GET", "/t/globex/folded", b"", 200, "0")?;
            gate.open()?;
            let answered = asked.join().map_err(|_| JourneyFailed::Panicked)??;
            assert_eq!(answered, (200, String::from("acme:None:x")));
            client.answers("GET", "/t/acme/folded", b"", 200, "1")
        })
    })?;
    assert_eq!(served.load(Ordering::Acquire), 1);
    Ok(())
}

/// Catches a client left waiting past the deadline on a hung service, an edge whose
/// liveness dies with it, and a late answer lost: it folds after the 504.
#[test]
fn a_hung_service_answers_504_and_its_late_answer_still_folds()
-> Result<(), Box<dyn std::error::Error>> {
    let (provider, gate, served) = rig();
    serve_app(held_app(provider, Duration::from_millis(50)), |client| {
        let hung = client.exchange("POST", "/t/acme/do", b"\"x\"")?;
        assert_eq!(hung.0, 504, "{hung:?}");
        assert!(hung.1.contains("retry"), "{hung:?}");
        gate.arrived()?;
        client.answers("GET", "/healthz", b"", 200, "ok")?;
        client.answers("GET", "/t/acme/folded", b"", 200, "0")?;
        gate.open()?;
        while client.exchange("GET", "/t/acme/folded", b"")?.1 != "1" {
            std::thread::yield_now();
        }
        Ok(())
    })?;
    assert_eq!(served.load(Ordering::Acquire), 1);
    Ok(())
}
