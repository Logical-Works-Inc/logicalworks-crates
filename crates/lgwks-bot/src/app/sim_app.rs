//! Seeded simulation of the app SDK on its cell host (issue #791).
//!
//! Source: #791's acceptance ("an ask effect's answer reaches the named `on`
//! handler"; "an effect the app was not granted is refused, never
//! dispatched"; "a body that does not decode is answered 400") and the
//! effects-as-data rule that a transition's effects run outside the cell and
//! re-enter as messages. One seed drives the tenants, the bodies, the provider's
//! answers and the malformed requests. A model folds the same draws, and every
//! answer the host gives is checked against it and hashed, so the same seed
//! gives the same trace hash.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::seeded::{DrawError, Seeded};

use super::transition::{AppCell, AppMessage};
use super::{App, AppError, Effect, EffectResult, Method, Provider, ReplyTag};
use crate::cap::Cap;
use crate::cell::{CellError, CellHost, DynHost, ReplySlot};
use crate::ecs::Response;

/// Seeds the seeded properties run.
const SEEDS: u64 = 8;

/// One tenant's ledger: what its posts added and what its effects brought.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Ledger {
    /// The sum of every posted `n`.
    total: u64,
    /// Bytes the provider answered to this tenant's asks.
    answered: usize,
    /// Asks the provider could not serve.
    unavailable: u64,
    /// Fetches refused for want of the `net` cap.
    refused: u64,
}

impl Ledger {
    /// The ledger as `GET /ledger` renders it.
    fn render(&self) -> Vec<u8> {
        format!(
            "{},{},{},{}",
            self.total, self.answered, self.unavailable, self.refused
        )
        .into_bytes()
    }

    /// Fold one post of `n`, given what the provider answers an ask of `n`.
    fn post(&mut self, n: u64, answer: Option<usize>) {
        self.total = self.total.saturating_add(n);
        if n.is_multiple_of(3) {
            match answer {
                Some(bytes) => self.answered = self.answered.saturating_add(bytes),
                None => self.unavailable = self.unavailable.saturating_add(1),
            }
        }
        if n.is_multiple_of(5) {
            self.refused = self.refused.saturating_add(1);
        }
    }
}

/// A posted body.
#[derive(lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct Add {
    /// What to add.
    n: u64,
}

/// The provider: answers an ask of `n` with `seeded_answer(seed, n)` bytes,
/// or none, and counts every dispatch it saw.
struct SeededProvider {
    /// The run's seed, mixed into each answer.
    seed: u64,
    /// Dispatches that reached the provider.
    seen: Arc<AtomicU64>,
}

/// How many bytes the provider answers an ask of `n`, below 17: `None` one
/// ask in four.
fn seeded_answer(seed: u64, n: u64) -> Option<usize> {
    let mixed = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ n.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    let [low, ..] = (mixed % 17).to_le_bytes();
    match mixed % 4 {
        0 => None,
        _ => Some(usize::from(low)),
    }
}

impl Provider for SeededProvider {
    fn dispatch(&self, tenant: &str, effect: &Effect) -> Option<Vec<u8>> {
        self.seen.fetch_add(1, Ordering::Relaxed);
        let Effect::Ask { ref prompt, .. } = *effect else {
            lgwks_std::trace::debug!(tenant, ?effect, "sim_app: only asks reach the provider");
            return None;
        };
        let n = prompt.parse::<u64>().ok()?;
        Some(vec![b'a'; seeded_answer(self.seed, n)?])
    }
}

/// The app under simulation: `POST /add` folds `n`, asks when `n % 3 == 0`
/// and fetches when `n % 5 == 0`, though `net` is never granted.
fn ledger_app(provider: SeededProvider) -> App<Ledger> {
    App::new("sim-ledger")
        .state::<Ledger>()
        .grant(Cap::new("app.ask"))
        .provider(provider)
        .route(Method::POST, "/add", |ledger: &mut Ledger, add: Add| {
            ledger.total = ledger.total.saturating_add(add.n);
            let mut effects = vec![Effect::reply(200, ledger.total.to_string().into_bytes())];
            if add.n.is_multiple_of(3) {
                effects.push(Effect::Ask {
                    prompt: add.n.to_string().into(),
                    reply_to: ReplyTag::new("asked"),
                });
            }
            if add.n.is_multiple_of(5) {
                effects.push(Effect::Fetch {
                    url: "https://example.invalid/".into(),
                    reply_to: ReplyTag::new("fetched"),
                });
            }
            effects
        })
        .route(Method::GET, "/ledger", |ledger: &mut Ledger, (): ()| {
            vec![Effect::reply(200, ledger.render())]
        })
        .on(ReplyTag::new("asked"), |ledger: &mut Ledger, input| {
            match *input.result() {
                EffectResult::Value(ref bytes) => {
                    ledger.answered = ledger.answered.saturating_add(bytes.len());
                }
                EffectResult::Unavailable | EffectResult::Refused(_) => {
                    ledger.unavailable = ledger.unavailable.saturating_add(1);
                }
            }
            Vec::new()
        })
        .on(ReplyTag::new("fetched"), |ledger: &mut Ledger, input| {
            if matches!(*input.result(), EffectResult::Refused(_)) {
                ledger.refused = ledger.refused.saturating_add(1);
            }
            Vec::new()
        })
}

/// How a simulation broke the model.
#[derive(Debug)]
enum SimFailed {
    /// The seeded stream refused a draw.
    Draw(DrawError),
    /// The app could not start.
    App(AppError),
    /// The host refused a submission or a wait.
    Host(CellError),
    /// An answer differs from the model.
    Answer {
        /// The tenant whose request it was.
        tenant: String,
        /// What the model allows.
        expected: (u16, Vec<u8>),
        /// What the host answered.
        found: (u16, Vec<u8>),
    },
    /// Two traces that must agree differ, or two that must differ agree.
    Trace {
        /// The seed the traces were drawn from.
        seed: u64,
        /// Which property broke.
        property: &'static str,
    },
}

impl std::fmt::Display for SimFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Draw(cause) => write!(f, "draw: {cause}"),
            Self::App(ref cause) => write!(f, "the app could not start: {cause}"),
            Self::Host(ref cause) => write!(f, "the host refused: {cause}"),
            Self::Answer {
                ref tenant,
                ref expected,
                ref found,
            } => write!(
                f,
                "{tenant} answered {found:?}, the model allows {expected:?}"
            ),
            Self::Trace { seed, property } => write!(f, "seed {seed}: {property}"),
        }
    }
}

/// Log `failure` and hand it back, so every broken property leaves a record.
fn fail<T>(failure: SimFailed) -> Result<T, SimFailed> {
    lgwks_std::trace::warn!(%failure, "sim_app: a property broke");
    Err(failure)
}

/// One seeded draw below `bound`.
fn draw(stream: &mut Seeded, bound: u64) -> Result<u64, SimFailed> {
    match stream.below(bound) {
        Ok(value) => Ok(value),
        Err(cause) => fail(SimFailed::Draw(cause)),
    }
}

/// Send one request and wait for its answer. The runner feeds every effect's
/// answer back before the reply settles, so the next request for the tenant
/// queues behind it.
fn call(
    host: &CellHost<AppCell<Ledger>>,
    tenant: &str,
    method: Method,
    path: &str,
    body: &[u8],
) -> Result<Response, SimFailed> {
    let message = AppMessage::Route {
        tenant: tenant.into(),
        method,
        path: path.into(),
        body: body.into(),
        principal: None,
    };
    let reply = Arc::new(ReplySlot::new());
    let sent = host
        .submit_wait(tenant, message, Arc::clone(&reply))
        .and_then(|()| reply.wait_blocking());
    match sent {
        Ok(response) => Ok(response),
        Err(cause) => fail(SimFailed::Host(cause)),
    }
}

/// Require `response` to be `status` carrying `body`, then hash it.
fn expect(
    hasher: &mut Hasher,
    tenant: &str,
    response: Response,
    status: u16,
    body: &[u8],
) -> Result<(), SimFailed> {
    let found = (response.status(), response.into_body());
    if found.0 != status || found.1 != body {
        return fail(SimFailed::Answer {
            tenant: tenant.to_owned(),
            expected: (status, body.to_vec()),
            found,
        });
    }
    hasher.update(tenant.as_bytes());
    hasher.update(&found.0.to_be_bytes());
    hasher.update(&found.1);
    Ok(())
}

/// One seeded run over 8 to 39 tenants and 400 to 799 requests, an eighth
/// of them malformed or misrouted. Returns the hash over every answer.
fn run(seed: u64) -> Result<Digest, SimFailed> {
    let mut stream = Seeded::from_seed(seed);
    let tenants = draw(&mut stream, 32)?.saturating_add(8);
    let operations = draw(&mut stream, 400)?.saturating_add(400);
    let seen = Arc::new(AtomicU64::new(0));
    let provider = SeededProvider {
        seed,
        seen: Arc::clone(&seen),
    };
    let host = match ledger_app(provider).start(4, 1_024) {
        Ok(host) => host,
        Err(cause) => return fail(SimFailed::App(cause)),
    };
    let mut model: BTreeMap<String, Ledger> = BTreeMap::new();
    let (mut hasher, mut asks) = (Hasher::new(), 0_u64);
    for _ in 0..operations {
        let tenant = format!("tenant-{}", draw(&mut stream, tenants)?);
        let ledger = model.entry(tenant.clone()).or_default();
        match draw(&mut stream, 16)? {
            0 => {
                let response = call(&host, &tenant, Method::POST, "/add", b"{\"n\":")?;
                let refusal = b"the body does not decode as this route's input";
                expect(&mut hasher, &tenant, response, 400, refusal)?;
            }
            1 => {
                let response = call(&host, &tenant, Method::POST, "/nowhere", b"{}")?;
                expect(&mut hasher, &tenant, response, 404, b"no such route")?;
            }
            _ => {
                let n = draw(&mut stream, 64)?;
                let body = format!("{{\"n\":{n}}}");
                let response = call(&host, &tenant, Method::POST, "/add", body.as_bytes())?;
                ledger.post(n, seeded_answer(seed, n));
                asks = asks.saturating_add(u64::from(n.is_multiple_of(3)));
                expect(
                    &mut hasher,
                    &tenant,
                    response,
                    200,
                    ledger.total.to_string().as_bytes(),
                )?;
            }
        }
    }
    for (tenant, ledger) in &model {
        let response = call(&host, tenant, Method::GET, "/ledger", b"")?;
        expect(&mut hasher, tenant, response, 200, &ledger.render())?;
    }
    host.close();
    if seen.load(Ordering::Relaxed) != asks {
        return fail(SimFailed::Trace {
            seed,
            property: "the provider saw an effect other than the granted asks",
        });
    }
    Ok(hasher.finalize())
}

/// Every answer matches the model, and the same seed replays the same trace
/// hash.
#[test]
fn same_seed_replays_the_same_trace_hash() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        if run(seed)? != run(seed)? {
            return fail(SimFailed::Trace {
                seed,
                property: "the trace hash did not replay",
            });
        }
    }
    Ok(())
}

/// Two seeds draw two different traces.
#[test]
fn distinct_seeds_diverge() -> Result<(), SimFailed> {
    if run(1)? == run(2)? {
        return fail(SimFailed::Trace {
            seed: 2,
            property: "two seeds share a trace hash",
        });
    }
    Ok(())
}

/// One seeded plan: up to three routes on a seeded verb and status, cut in
/// half one time in eight.
fn seeded_plan(stream: &mut Seeded) -> Result<Vec<u8>, SimFailed> {
    let mut routes = Vec::new();
    for at in 0..draw(stream, 4)? {
        let method = match draw(stream, 3)? {
            0 => "GET",
            1 => "POST",
            _ => "PUT",
        };
        let status = draw(stream, 700)?;
        routes.push(format!(
            "{{\"method\":\"{method}\",\"path\":\"/r{at}\",\"status\":{status},\"body\":\"ok\"}}"
        ));
    }
    let mut bytes = format!("{{\"name\":\"plan\",\"routes\":[{}]}}", routes.join(",")).into_bytes();
    if draw(stream, 8)? == 0 {
        bytes.truncate(bytes.len() >> 1);
    }
    Ok(bytes)
}

/// A plan of seeded bytes is built or refused with a typed error, never a
/// panic, and the same bytes always get the same verdict.
#[test]
fn seeded_plans_are_built_or_refused_never_panic() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        let mut stream = Seeded::from_seed(seed);
        for _ in 0..64 {
            let bytes = seeded_plan(&mut stream)?;
            let first = App::<()>::from_plan(&bytes)
                .map(drop)
                .map_err(|error| error.to_string());
            let again = App::<()>::from_plan(&bytes)
                .map(drop)
                .map_err(|error| error.to_string());
            if first != again {
                return fail(SimFailed::Trace {
                    seed,
                    property: "one plan got two verdicts",
                });
            }
        }
    }
    Ok(())
}
