//! Seeded simulation of the per-tenant cell host (issue #790).
//!
//! Source: #790's acceptance ("a panicking transition: that cell rebuilds
//! from its log, no other cell is affected"; "same seed, same trace hash";
//! "two tenants on the same resource ids: zero cross-tenant reads") and the
//! event-sourcing recovery rule that a snapshot plus the events after it fold
//! to the state the events alone would. One seed drives the tenants, the
//! methods and the poisoned messages; a few hot tenants take enough traffic to
//! cross many checkpoints. The same seed gives the same trace hash, and a run
//! with every poisoned message removed gives the same hash as a run that sent
//! them, so a contained panic is invisible to every answer that follows.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lgwks_std::hash::{Digest, Hasher};
use lgwks_std::seeded::{DrawError, Seeded};

use super::CHECKPOINT_EVERY;
use crate::ecs::{Request, RequestFeed, RequestRefused, Response};

/// Seeds the seeded properties run.
const SEEDS: u64 = 32;

/// Hot tenants: one draw in eight goes to one of these, so their tails cross
/// the checkpoint many times in one run.
const HOT_TENANTS: u64 = 4;

/// A per-tenant counter: `POST /count` adds one, `GET /count` reads it, and a
/// body of `boom` panics inside the cell.
fn serve_counter(request: &Request<'_>, count: &mut u64) -> Response {
    assert!(
        request.body() != b"boom",
        "the handler panics on a poisoned message"
    );
    if request.method() == "POST" {
        *count = count.saturating_add(1);
    }
    Response::new(200, count.to_string().into_bytes())
}

/// How a simulation broke the model.
#[derive(Debug)]
enum SimFailed {
    /// The seeded stream refused a draw.
    Draw(DrawError),
    /// The host refused an answer.
    Answer(RequestRefused),
    /// A tenant read a count its own requests did not make.
    Read {
        /// The tenant that read wrong.
        tenant: String,
        /// What the tenant holds in the model.
        expected: u64,
        /// What the host answered.
        found: Vec<u8>,
    },
    /// A request answered a status the model does not allow.
    Status {
        /// The tenant whose request it was.
        tenant: String,
        /// The status the model allows.
        expected: u16,
        /// The status the host answered.
        found: u16,
    },
    /// The host's counts disagree with the answers the run saw.
    Counts {
        /// What the host reports, rendered.
        counts: String,
        /// Replies the run collected.
        replies: u64,
        /// Poisoned messages the run sent.
        booms: u64,
        /// Tenants the model holds.
        tenants: usize,
    },
    /// Two traces that must agree differ, or two that must differ agree.
    Trace {
        /// The seed the traces were drawn from.
        seed: u64,
        /// Which property broke.
        property: &'static str,
    },
    /// A seeded run failed.
    Seed {
        /// The seed that failed.
        seed: u64,
        /// How it failed.
        cause: Box<SimFailed>,
    },
}

impl std::fmt::Display for SimFailed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Draw(cause) => write!(f, "draw: {cause}"),
            Self::Answer(refusal) => write!(f, "the host refused an answer: {refusal}"),
            Self::Read {
                ref tenant,
                expected,
                ref found,
            } => write!(f, "{tenant} read {found:?}, the model holds {expected}"),
            Self::Status {
                ref tenant,
                expected,
                found,
            } => write!(f, "{tenant} answered {found}, the model allows {expected}"),
            Self::Counts {
                ref counts,
                replies,
                booms,
                tenants,
            } => write!(
                f,
                "counts {counts} against {replies} replies, {booms} poisons, {tenants} tenants"
            ),
            Self::Trace { seed, property } => write!(f, "seed {seed}: {property}"),
            Self::Seed { seed, ref cause } => write!(f, "seed {seed}: {cause}"),
        }
    }
}

/// Log `failure` and hand it back, so every broken property leaves a record.
fn fail<T>(failure: SimFailed) -> Result<T, SimFailed> {
    lgwks_std::trace::warn!(%failure, "sim_cell: a property broke");
    Err(failure)
}

/// One seeded draw below `bound`.
fn draw(stream: &mut Seeded, bound: u64) -> Result<u64, SimFailed> {
    stream.below(bound).map_err(SimFailed::Draw)
}

/// Answer one request on `feed`.
fn ask(feed: &RequestFeed, tenant: &str, method: &str, body: &[u8]) -> Result<Response, SimFailed> {
    let request = Request::new(tenant, method, "/count", body).map_err(SimFailed::Answer)?;
    feed.answer(&request).map_err(SimFailed::Answer)
}

/// Require `response` to be a `200` carrying `count`.
fn expect_count(tenant: &str, response: Response, count: u64) -> Result<(), SimFailed> {
    if response.status() != 200 {
        return fail(SimFailed::Status {
            tenant: tenant.to_owned(),
            expected: 200,
            found: response.status(),
        });
    }
    let found = response.into_body();
    if found != count.to_string().into_bytes() {
        return fail(SimFailed::Read {
            tenant: tenant.to_owned(),
            expected: count,
            found,
        });
    }
    Ok(())
}

/// Require a poisoned message to have been answered `500`.
fn expect_panicked(tenant: &str, response: &Response) -> Result<(), SimFailed> {
    if response.status() != 500 {
        return fail(SimFailed::Status {
            tenant: tenant.to_owned(),
            expected: 500,
            found: response.status(),
        });
    }
    Ok(())
}

/// One seeded run over 1,000 to 2,000 tenants and 2,000 to 4,000 requests.
/// With `poison`, one post in 64 carries `boom` and must answer `500`;
/// without it the same draws are made and the poisoned post is not sent.
/// Returns the hash over every non-poisoned answer, in order.
fn run(seed: u64, poison: bool) -> Result<Digest, SimFailed> {
    let mut stream = Seeded::from_seed(seed);
    let tenants = draw(&mut stream, 1_000)?.saturating_add(1_000);
    let operations = draw(&mut stream, 2_000)?.saturating_add(2_000);
    let feed = RequestFeed::new(serve_counter);
    let mut model: BTreeMap<String, u64> = BTreeMap::new();
    let mut hasher = Hasher::new();
    let (mut replies, mut booms) = (0_u64, 0_u64);
    for _ in 0..operations {
        let tenant = if draw(&mut stream, 8)? == 0 {
            format!("hot-{}", draw(&mut stream, HOT_TENANTS)?)
        } else {
            format!("tenant-{}", draw(&mut stream, tenants)?)
        };
        let post = draw(&mut stream, 4)? != 0;
        let poisoned = draw(&mut stream, 64)? == 0 && post;
        if poisoned {
            if poison {
                let response = ask(&feed, &tenant, "POST", b"boom")?;
                replies = replies.saturating_add(1);
                booms = booms.saturating_add(1);
                expect_panicked(&tenant, &response)?;
            }
            continue;
        }
        let method = if post { "POST" } else { "GET" };
        let response = ask(&feed, &tenant, method, b"")?;
        replies = replies.saturating_add(1);
        let count = model.entry(tenant.clone()).or_default();
        if post {
            *count = count.saturating_add(1);
        }
        hasher.update(tenant.as_bytes());
        hasher.update(method.as_bytes());
        hasher.update(count.to_string().as_bytes());
        expect_count(&tenant, response, *count)?;
    }
    let counts = feed.counts();
    // A tenant whose only message panicked was made by that message, so the
    // host may hold more tenants than the model, never fewer.
    if counts.settled != replies
        || counts.panicked != booms
        || counts.refused != 0
        || counts.tenants < model.len()
    {
        return fail(SimFailed::Counts {
            counts: format!("{counts:?}"),
            replies,
            booms,
            tenants: model.len(),
        });
    }
    Ok(hasher.finalize())
}

/// Run `seed`, naming it in the failure.
fn seeded(seed: u64, poison: bool) -> Result<Digest, SimFailed> {
    run(seed, poison).map_err(|cause| SimFailed::Seed {
        seed,
        cause: Box::new(cause),
    })
}

/// The same seed replays the same trace hash. Cites #790: "same seed, same
/// trace hash".
#[test]
fn same_seed_replays_the_same_trace_hash() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        if seeded(seed, true)? != seeded(seed, true)? {
            return fail(SimFailed::Trace {
                seed,
                property: "the trace hash did not replay",
            });
        }
    }
    Ok(())
}

/// Distinct seeds diverge: the trace is the run, not a constant.
#[test]
fn distinct_seeds_diverge() -> Result<(), SimFailed> {
    let mut hashes: Vec<(u64, Digest)> = Vec::new();
    for seed in 0..16_u64 {
        let hash = seeded(seed, true)?;
        if hashes.iter().any(|earlier| earlier.1 == hash) {
            return fail(SimFailed::Trace {
                seed,
                property: "the trace hash repeats an earlier seed's",
            });
        }
        hashes.push((seed, hash));
    }
    Ok(())
}

/// A contained panic is invisible: the run that sent every poisoned message
/// answers everything else exactly as the run that never sent them, and the
/// host counts one panic per poison and one answer per request. Cites #790:
/// "that cell rebuilds from its log, no other cell is affected, and the panic
/// is a recorded outcome".
#[test]
fn a_panic_is_invisible_in_the_trace() -> Result<(), SimFailed> {
    for seed in 0..SEEDS {
        if seeded(seed, true)? != seeded(seed, false)? {
            return fail(SimFailed::Trace {
                seed,
                property: "a contained panic changed the trace",
            });
        }
    }
    Ok(())
}

/// A panic at every offset around the first two checkpoints rebuilds the
/// exact count: the snapshot plus its tail is the state. Each offset is its
/// own tenant on one feed.
#[test]
fn a_panic_at_every_checkpoint_offset_rebuilds_the_exact_state() -> Result<(), SimFailed> {
    let feed = RequestFeed::new(serve_counter);
    let mut span = 0_u64;
    for _ in 0..CHECKPOINT_EVERY.saturating_mul(2).saturating_add(2) {
        span = span.saturating_add(1);
    }
    for posts in 0..span {
        let tenant = format!("offset-{posts}");
        for _ in 0..posts {
            ask(&feed, &tenant, "POST", b"")?;
        }
        expect_panicked(&tenant, &ask(&feed, &tenant, "POST", b"boom")?)?;
        expect_count(&tenant, ask(&feed, &tenant, "GET", b"")?, posts)?;
    }
    let counts = feed.counts();
    if counts.panicked != span {
        return fail(SimFailed::Counts {
            counts: format!("{counts:?}"),
            replies: counts.settled,
            booms: span,
            tenants: counts.tenants,
        });
    }
    Ok(())
}

/// A tenant that has lived through hundreds of checkpoints still rebuilds
/// exactly: the log stays bounded, so a long-lived tenant is never refused
/// for the size of its history.
#[test]
fn a_long_lived_tenant_still_rebuilds_after_ten_thousand_messages() -> Result<(), SimFailed> {
    let feed = RequestFeed::new(serve_counter);
    for _ in 0..10_000_u32 {
        ask(&feed, "elder", "POST", b"")?;
    }
    expect_panicked("elder", &ask(&feed, "elder", "POST", b"boom")?)?;
    expect_count("elder", ask(&feed, "elder", "GET", b"")?, 10_000)
}

/// A logged message that panics only when replayed (a handler that is not a
/// pure function of its state and message) is dropped and the rebuild
/// converges, rather than looping on it.
#[test]
fn a_message_that_panics_only_in_replay_is_dropped_and_the_rebuild_converges()
-> Result<(), SimFailed> {
    let armed = Arc::new(AtomicBool::new(false));
    let trap = Arc::clone(&armed);
    let feed = RequestFeed::new(move |request: &Request<'_>, count: &mut u64| {
        assert!(
            request.body() != b"boom",
            "the handler panics on a poisoned message"
        );
        if request.body() == b"late" {
            assert!(
                !trap.load(Ordering::Acquire),
                "the late message panics once armed"
            );
            *count = count.saturating_add(10);
        } else if request.method() == "POST" {
            *count = count.saturating_add(1);
        }
        Response::new(200, count.to_string().into_bytes())
    });
    ask(&feed, "flaky", "POST", b"")?;
    expect_count("flaky", ask(&feed, "flaky", "POST", b"late")?, 11)?;
    armed.store(true, Ordering::Release);
    expect_panicked("flaky", &ask(&feed, "flaky", "POST", b"boom")?)?;
    expect_count("flaky", ask(&feed, "flaky", "GET", b"")?, 1)
}

/// A panicking tenant never changes another tenant's state: one tenant takes
/// a poison every third round while the other counts exactly.
#[test]
fn a_panicking_tenant_never_changes_another_tenants_state() -> Result<(), SimFailed> {
    let feed = RequestFeed::new(serve_counter);
    for round in 0..100_u64 {
        if round.rem_euclid(3) == 0 {
            expect_panicked("hot", &ask(&feed, "hot", "POST", b"boom")?)?;
        } else {
            let expected = round.saturating_sub(round.div_euclid(3));
            expect_count("hot", ask(&feed, "hot", "POST", b"")?, expected)?;
        }
        expect_count(
            "cold",
            ask(&feed, "cold", "POST", b"")?,
            round.saturating_add(1),
        )?;
    }
    Ok(())
}

/// Two tenants with identical resource ids never read each other: the same
/// path under two names counts separately. Cites #790: "zero cross-tenant
/// reads, by construction and by test".
#[test]
fn two_tenants_with_identical_resource_ids_never_read_each_other() -> Result<(), SimFailed> {
    let feed = RequestFeed::new(serve_counter);
    for _ in 0..2_u64 {
        ask(&feed, "left", "POST", b"")?;
    }
    for _ in 0..5_u64 {
        ask(&feed, "right", "POST", b"")?;
    }
    expect_count("left", ask(&feed, "left", "GET", b"")?, 2)?;
    expect_count("right", ask(&feed, "right", "GET", b"")?, 5)
}
