//! Seeded simulation of the per-tenant budget on the app's host (#1042, #1044).
//!
//! Source: #1042's ask for a per-tenant rate limit through `App`, and the quota
//! policy of the IETF HTTP API working group's RateLimit header fields draft
//! (draft-ietf-httpapi-ratelimit-headers): a quota of `q` units per window of `w`,
//! kept per partition key. Here the key is the tenant, a unit is one effect the
//! dispatcher serves, and the window is fixed and opened by the tenant's first
//! charge. One seed drives the tenants, the asks, the allowance and every step of
//! the virtual clock. A model folds the same draws. Every answer is checked
//! against it and hashed, so the same seed gives the same trace hash.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::seeded::{DrawError, Seeded};

use super::budget::{BUDGET_CAP, BudgetLimits, Charge, Ledger};
use super::sim_command::{Host, Outcome, Service, book_app, book_of, roomy, run_command, said};
use super::transition::{AppCell, AppMessage};
use super::{App, AppError, Effect, EffectResult, Method, Principal, Provider, ReplyTag};
use crate::cap::Cap;
use crate::cell::{CellError, CellHost, DynHost, ReplySlot};
use crate::clock::{Clock, ClockError};
use crate::ecs::Response;

/// Seeds the seeded properties run.
const SEEDS: u64 = 8;

/// The window every simulated budget uses.
const WINDOW: Duration = Duration::from_secs(1);

/// One tenant's fold of how its asks were answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Tally {
    /// Asks the provider served.
    served: u64,
    /// Asks refused for the budget.
    refused: u64,
    /// Any other answer, which this app never earns.
    other: u64,
}

impl Tally {
    /// The tally as `GET /tally` renders it.
    fn render(&self) -> Vec<u8> {
        format!("{},{},{}", self.served, self.refused, self.other).into_bytes()
    }
}

/// A posted body: how many asks to send.
#[derive(lgwks_std::json::Deserialize)]
#[serde(crate = "lgwks_std::json::serde", deny_unknown_fields)]
struct Asks {
    /// The count.
    count: u64,
}

/// The served count per tenant, under one lock.
type Seen = Arc<Mutex<BTreeMap<String, u64>>>;

/// The counts, poisoned or not: each step under the lock is one add.
fn counts(seen: &Seen) -> MutexGuard<'_, BTreeMap<String, u64>> {
    match seen.lock() {
        Ok(held) => held,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// A provider that serves every ask and counts, per tenant, what it served.
struct Counting {
    /// What reached the provider.
    seen: Seen,
}

impl Provider for Counting {
    fn dispatch(&self, tenant: &str, _: &Effect) -> Option<Vec<u8>> {
        let mut seen = counts(&self.seen);
        let count = seen.entry(tenant.to_owned()).or_default();
        *count = count.saturating_add(1);
        Some(b"ok".to_vec())
    }
}

/// The app under simulation: `POST /ask` sends `k` asks, and `GET /tally` reads
/// how they were answered.
fn metered_app(provider: Counting, limits: BudgetLimits, clock: Clock) -> App<Tally> {
    App::new("sim-budget")
        .state::<Tally>()
        .grant(Cap::new("app.ask"))
        .provider(provider)
        .budget(limits, clock)
        .route(Method::POST, "/ask", |_: &mut Tally, asks: Asks| {
            let mut effects = vec![Effect::reply(200, b"sent".to_vec())];
            for at in 0..asks.count {
                effects.push(Effect::Ask {
                    prompt: at.to_string().into(),
                    reply_to: ReplyTag::new("asked"),
                });
            }
            effects
        })
        .route(Method::GET, "/tally", |tally: &mut Tally, (): ()| {
            vec![Effect::reply(200, tally.render())]
        })
        .on(ReplyTag::new("asked"), |tally: &mut Tally, input| {
            match *input.result() {
                EffectResult::Value(_) => tally.served = tally.served.saturating_add(1),
                EffectResult::Refused(ref cap) if cap.as_str() == BUDGET_CAP => {
                    tally.refused = tally.refused.saturating_add(1);
                }
                EffectResult::Refused(_) | EffectResult::Unavailable => {
                    tally.other = tally.other.saturating_add(1);
                }
            }
            Vec::new()
        })
}

/// The model of one tenant's fixed window.
#[derive(Debug, Clone, Copy, Default)]
struct Window {
    /// When it opened, if it has.
    start: Option<Duration>,
    /// Units spent in it.
    used: u32,
}

impl Window {
    /// Whether a charge at `now` under `allowance` is served.
    fn charge(&mut self, now: Duration, allowance: u32) -> bool {
        let reopens = match self.start {
            Some(start) => now.saturating_sub(start) >= WINDOW,
            None => true,
        };
        if reopens {
            self.start = Some(now);
            self.used = 0;
        }
        let served = self.used < allowance;
        self.used = self.used.saturating_add(u32::from(served));
        served
    }
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
    /// The virtual clock refused a step.
    Clock(ClockError),
    /// An answer differs from the model.
    Answer {
        /// The tenant whose request it was.
        tenant: String,
        /// What the model allows.
        expected: (u16, Vec<u8>),
        /// What the host answered.
        found: (u16, Vec<u8>),
    },
    /// A property over the whole run broke.
    Trace {
        /// The seed the run was drawn from.
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
            Self::Clock(ref cause) => write!(f, "the clock refused: {cause}"),
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
    lgwks_std::trace::warn!(%failure, "sim_budget: a property broke");
    Err(failure)
}

/// One seeded draw below `bound`.
fn draw(stream: &mut Seeded, bound: u64) -> Result<u64, SimFailed> {
    stream
        .below(bound)
        .or_else(|cause| fail(SimFailed::Draw(cause)))
}

/// Send one request and wait for its answer. Every effect is charged and its
/// answer fed back before the reply settles.
fn call(
    host: &CellHost<AppCell<Tally>>,
    tenant: &str,
    (method, path): (Method, &str),
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
    host.submit_wait(tenant, message, Arc::clone(&reply))
        .and_then(|()| reply.wait_blocking())
        .or_else(|cause| fail(SimFailed::Host(cause)))
}

/// Require `response` to be `status` carrying `body`, then hash it.
fn expect(
    hasher: &mut Hasher,
    tenant: &str,
    response: Response,
    (status, body): (u16, &[u8]),
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

/// One seeded run over 3 to 10 tenants and 200 to 399 steps. A sixth of the
/// steps move the clock by up to 1.5 windows; the rest post 0 to 4 asks.
fn run(seed: u64) -> Result<Digest, SimFailed> {
    let mut stream = Seeded::from_seed(seed);
    let tenants = draw(&mut stream, 8)?.saturating_add(3);
    let steps = draw(&mut stream, 200)?.saturating_add(200);
    let allowance = match draw(&mut stream, 4)? {
        0 => 1,
        1 => 2,
        2 => 3,
        _ => 5,
    };
    let clock = Clock::virtual_at(Duration::ZERO);
    let seen = Seen::default();
    let provider = Counting {
        seen: Arc::clone(&seen),
    };
    let limits = BudgetLimits::new(allowance, WINDOW, 64);
    let host = metered_app(provider, limits, clock.clone())
        .start(4, 1_024)
        .or_else(|cause| fail(SimFailed::App(cause)))?;
    let mut model: BTreeMap<String, (Window, Tally)> = BTreeMap::new();
    let mut hasher = Hasher::new();
    for _ in 0..steps {
        if draw(&mut stream, 6)? == 0 {
            let step = draw(&mut stream, 1_500)?;
            clock
                .advance(Duration::from_millis(step))
                .or_else(|cause| fail(SimFailed::Clock(cause)))?;
            hasher.update(&step.to_be_bytes());
            continue;
        }
        let tenant = format!("tenant-{}", draw(&mut stream, tenants)?);
        let asks = draw(&mut stream, 5)?;
        let body = format!("{{\"count\":{asks}}}");
        let response = call(&host, &tenant, (Method::POST, "/ask"), body.as_bytes())?;
        expect(&mut hasher, &tenant, response, (200, b"sent"))?;
        let &mut (ref mut window, ref mut tally) = model.entry(tenant).or_default();
        for _ in 0..asks {
            if window.charge(clock.now(), allowance) {
                tally.served = tally.served.saturating_add(1);
            } else {
                tally.refused = tally.refused.saturating_add(1);
            }
        }
    }
    let (mut served, mut refused) = (0_u64, 0_u64);
    for (tenant, entry) in &model {
        let tally = &entry.1;
        let response = call(&host, tenant, (Method::GET, "/tally"), b"")?;
        expect(&mut hasher, tenant, response, (200, &tally.render()))?;
        if counts(&seen).get(tenant).copied() != Some(tally.served).filter(|&any| any > 0) {
            return fail(SimFailed::Trace {
                seed,
                property: "the provider served a tenant other than its budget allowed",
            });
        }
        served = served.saturating_add(tally.served);
        refused = refused.saturating_add(tally.refused);
    }
    host.close();
    if served == 0 || refused == 0 {
        return fail(SimFailed::Trace {
            seed,
            property: "the run never both served and refused, so the budget was never tried",
        });
    }
    Ok(hasher.finalize())
}

/// Every answer matches the model, refused asks never reach the provider, and
/// the same seed replays the same trace hash.
#[test]
fn sim_budget_same_seed_replays_the_same_trace_hash() -> Result<(), SimFailed> {
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
fn sim_budget_distinct_seeds_diverge() -> Result<(), SimFailed> {
    if run(1)? == run(2)? {
        return fail(SimFailed::Trace {
            seed: 2,
            property: "two seeds share a trace hash",
        });
    }
    Ok(())
}

/// A ledger over `allowance` per second for at most `tenants`, on a virtual clock.
fn ledger(allowance: u32, tenants: usize) -> (Ledger, Clock) {
    let clock = Clock::virtual_at(Duration::ZERO);
    let limits = BudgetLimits::new(allowance, WINDOW, tenants);
    (Ledger::new(limits, clock.clone()), clock)
}

/// The window reopens at exactly its length, not one tick before.
#[test]
fn sim_budget_a_window_reopens_exactly_when_it_ends() -> Outcome {
    let (ledger, clock) = ledger(1, 4);
    assert_eq!(ledger.charge("edge"), Charge::Granted, "the first unit");
    clock.advance(Duration::from_millis(999))?;
    assert_eq!(
        ledger.charge("edge"),
        Charge::Spent,
        "a tick before the end"
    );
    clock.advance(Duration::from_millis(1))?;
    assert_eq!(ledger.charge("edge"), Charge::Granted, "at the end");
    Ok(())
}

/// One tenant spending its allowance leaves every other tenant's whole.
#[test]
fn sim_budget_one_tenant_never_spends_another() {
    let (ledger, _clock) = ledger(2, 4);
    for expected in [Charge::Granted, Charge::Granted, Charge::Spent] {
        assert_eq!(ledger.charge("noisy"), expected, "the noisy tenant");
    }
    assert_eq!(ledger.charge("quiet"), Charge::Granted, "the quiet tenant");
    assert_eq!(ledger.charge("quiet"), Charge::Granted, "its second unit");
}

/// A full table refuses a new tenant while every window is live, never evicts a
/// live charge, and admits the new tenant once the windows end.
#[test]
fn sim_budget_a_full_table_refuses_a_new_tenant_until_a_window_ends() -> Outcome {
    let (ledger, clock) = ledger(1, 2);
    assert_eq!(ledger.charge("first"), Charge::Granted, "the first tenant");
    assert_eq!(
        ledger.charge("second"),
        Charge::Granted,
        "the second tenant"
    );
    assert_eq!(ledger.charge("third"), Charge::TableFull, "no room");
    clock.advance(Duration::from_millis(500))?;
    assert_eq!(
        ledger.charge("third"),
        Charge::TableFull,
        "both windows live"
    );
    assert_eq!(ledger.charge("first"), Charge::Spent, "kept, not evicted");
    assert_eq!(ledger.tracked(), 2, "the table holds its bound");
    clock.advance(Duration::from_millis(500))?;
    assert_eq!(
        ledger.charge("third"),
        Charge::Granted,
        "the ended windows went"
    );
    assert_eq!(ledger.tracked(), 1, "only the new window is held");
    Ok(())
}

/// A name that is no tenant keys nothing and is refused.
#[test]
fn sim_budget_a_name_that_is_no_tenant_keys_nothing() {
    let (ledger, _clock) = ledger(8, 4);
    let long = "t".repeat(65);
    for name in ["", "two words", "../up", long.as_str()] {
        assert_eq!(ledger.charge(name), Charge::BadTenant, "{name:?}");
    }
    assert_eq!(ledger.tracked(), 0, "nothing was keyed");
}

/// `check` refuses a budget with a zero window or zero tenants, and takes a zero
/// allowance, which refuses every effect.
#[test]
fn sim_budget_check_refuses_a_budget_that_cannot_run() {
    let clock = Clock::virtual_at(Duration::ZERO);
    let with = |limits: BudgetLimits| App::new("metered").budget(limits, clock.clone()).check();
    let no_window = with(BudgetLimits::new(1, Duration::ZERO, 4));
    assert!(
        matches!(no_window, Err(AppError::BadBudget)),
        "{no_window:?}"
    );
    let no_tenants = with(BudgetLimits::new(1, WINDOW, 0));
    assert!(
        matches!(no_tenants, Err(AppError::BadBudget)),
        "{no_tenants:?}"
    );
    assert!(
        with(BudgetLimits::new(0, WINDOW, 1)).is_ok(),
        "a zero allowance runs"
    );
    assert!(with(BudgetLimits::default()).is_ok(), "the default runs");
}

/// A command past the budget is refused before the pool, so the service never
/// runs it, and the next window serves it again.
#[test]
fn sim_budget_a_command_past_the_budget_never_runs() -> Outcome {
    let seen = Arc::new(Mutex::new(0_u64));
    let counted = Arc::clone(&seen);
    let service = Service(move |_: &str, _: Option<&Principal>, body: &[u8]| {
        let mut ran = match counted.lock() {
            Ok(held) => held,
            Err(poisoned) => poisoned.into_inner(),
        };
        *ran = ran.saturating_add(1);
        Some(body.to_vec())
    });
    let clock = Clock::virtual_at(Duration::ZERO);
    let limits = BudgetLimits::new(1, WINDOW, 4);
    let host = granted_with_budget(service, limits, clock.clone())?;
    let tenant = "commander";
    assert_eq!(
        run_command(&host, tenant, "one")?,
        said(200, b"one"),
        "the first runs"
    );
    assert_eq!(
        run_command(&host, tenant, "two")?,
        said(403, b"refused"),
        "over budget"
    );
    clock.advance(WINDOW)?;
    assert_eq!(
        run_command(&host, tenant, "three")?,
        said(200, b"three"),
        "a new window"
    );
    assert_eq!(book_of(&host, tenant)?, "2,0,1", "two landed, one refused");
    let ran = match seen.lock() {
        Ok(held) => *held,
        Err(poisoned) => *poisoned.into_inner(),
    };
    assert_eq!(ran, 2, "the refused command never reached the service");
    host.close();
    Ok(())
}

/// The command app of `sim_command`, granted commands, with a budget.
fn granted_with_budget(
    service: impl Provider,
    limits: BudgetLimits,
    clock: Clock,
) -> Outcome<Host> {
    Ok(book_app(service, roomy())
        .grant(Cap::new("app.command"))
        .budget(limits, clock)
        .start(4, 1 << 17)?)
}
