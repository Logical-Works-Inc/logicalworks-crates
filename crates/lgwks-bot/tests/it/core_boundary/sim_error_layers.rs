//! Hostile simulations of one typed error per layer (#990, ADR 0061).
//!
//! The chain under test is the real one: a `KernelError` rides up through
//! `EcsError`, into `FlowError::Bot`, and out as a `BotError` arm, and every hop
//! is the shipped `From` impl. One seed drives each family; the oracle is a
//! table written here by hand (which layer, which dispatch certainty, which
//! retry class) or a relation that must hold between two builds of one failure,
//! never the code's own answer read back. Each family returns a trace hash, runs
//! twice per seed, and must differ under another seed.
//!
//! This file holds the reference table and the helpers every family shares; the
//! families live beside it by fool:
//!
//! | Child | Families (fool) |
//! |---|---|
//! | `sim_variants` | every world, session and script variant lands in one arm (saboteur) |
//! | `sim_bytes` | hostile bytes, giant and empty fields, repeated conversion, old versions (weird-bytes, giant-and-empty, double-clicker, fat-finger) |
//! | `sim_walks` | the `source()` walk, the route-to-arm bijection, two tenants, random layer operations (wrong-order, neighbour, monkey) |
//! | `sim_upstream` | lying upstreams, a killed run, a clock that steps (liar-upstream, pulled-plug, clock-fool) |

use std::error::Error;
use std::io;
use std::sync::Arc;
use std::time::Duration;

use lgwks_bot::app_error::BotError;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::{DispatchCertainty, KernelError, RetryClass};
use lgwks_bot::script::FlowError;
use lgwks_bot::session::SessionError;
use lgwks_bot::task::StoreError;

use super::layer_errors::{hostile, layer_of};
use crate::sim::Rng;

mod sim_bytes;
mod sim_upstream;
mod sim_variants;
mod sim_walks;

/// A test's result: an error fails it with the error's text.
type TestResult = Result<(), Box<dyn Error>>;

/// What one family returns: its trace hash, or the reason it could not run.
type Hash = Result<u64, Box<dyn Error>>;

/// One family: a seed in, a replay receipt out.
type Sim = fn(u64) -> Hash;

/// A family's refusal, emitted before it is returned so a caller has a signal
/// and not only a value.
fn refuse<T>(cause: impl Into<String>) -> Result<T, Box<dyn Error>> {
    let refusal: Result<T, Box<dyn Error>> = Err(cause.into().into());
    lgwks_std::trace::debug!(
        error = ?refusal.as_ref().err(),
        "family: returning an error to the caller"
    );
    refusal
}

/// The seeds every family replays.
const SEEDS: [u64; 4] = [
    0x990_5eed_0000_0001,
    0x990_5eed_0000_0002,
    0x990_5eed_ffff_0003,
    0x1990_5eed_0000_0004,
];

/// Failures drawn per seed.
const DRAWS: u32 = 96;

/// The field size the giant family treats as the limit.
const LIMIT: usize = 1 << 16;

/// The most links a `source()` walk may take before it is a loop.
const WALK_CAP: usize = 64;

// ── The reference table ────────────────────────────────────────────────────

/// A kernel failure's shape; the answer each gives is [`Kind::answer`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// A `DomainError` that refused the input.
    Refused,
    /// A `DomainError` that never reached the peer.
    NotDelivered,
    /// A `DomainError` that claims it is unsettled.
    Unsettled,
    /// A `DomainError` that claims the effect occurred.
    Occurred,
    /// An `EffectIndeterminate`.
    Indeterminate,
    /// A lost compare-and-set.
    Conflict,
    /// A lapsed credential.
    Expired,
}

impl Kind {
    /// A shape drawn from `rng`.
    fn draw(rng: &mut Rng) -> Self {
        match rng.below(7) {
            0 => Self::Refused,
            1 => Self::NotDelivered,
            2 => Self::Unsettled,
            3 => Self::Occurred,
            4 => Self::Indeterminate,
            5 => Self::Conflict,
            _ => Self::Expired,
        }
    }

    /// What the vocabulary says this shape establishes and permits, written
    /// out by hand from the documented table and not read from the code.
    fn answer(self) -> (DispatchCertainty, RetryClass) {
        match self {
            Self::Refused | Self::Conflict | Self::Expired => {
                (DispatchCertainty::Refused, RetryClass::Never)
            }
            Self::NotDelivered => (DispatchCertainty::NotDelivered, RetryClass::Safe),
            Self::Unsettled | Self::Indeterminate => {
                (DispatchCertainty::Unsettled, RetryClass::RequiresEvidence)
            }
            Self::Occurred => (DispatchCertainty::Occurred, RetryClass::Never),
        }
    }

    /// The kernel failure of this shape, over `fields`.
    fn build(self, fields: &Fields) -> KernelError {
        let domain = fields.domain.clone();
        let cause = fields.cause.clone();
        let certainty = match self {
            Self::NotDelivered => DispatchCertainty::NotDelivered,
            Self::Unsettled => DispatchCertainty::Unsettled,
            Self::Occurred => DispatchCertainty::Occurred,
            Self::Refused | Self::Indeterminate | Self::Conflict | Self::Expired => {
                DispatchCertainty::Refused
            }
        };
        match self {
            Self::Refused | Self::NotDelivered | Self::Unsettled | Self::Occurred => {
                KernelError::DomainError {
                    domain,
                    certainty,
                    cause,
                }
            }
            Self::Indeterminate => KernelError::EffectIndeterminate { domain, cause },
            Self::Conflict => KernelError::Conflict {
                domain,
                record: cause,
                expected: fields.version,
                found: fields.version.saturating_add(1),
            },
            Self::Expired => KernelError::CredentialExpired {
                capabilities: Vec::new(),
                expired_at: fields.near,
                now: fields.far,
            },
        }
    }
}

/// The free text and numbers of a drawn failure.
#[derive(Clone)]
struct Fields {
    /// The domain a kernel failure names.
    domain: String,
    /// The cause it carries.
    cause: String,
    /// The first clock reading.
    near: Duration,
    /// The second clock reading.
    far: Duration,
    /// A version or a count.
    version: u64,
}

/// A clock reading the fool chooses: zero, one tick, the end of time, a year,
/// an hour, or noise.
fn reading(rng: &mut Rng) -> Duration {
    match rng.below(8) {
        0 => Duration::ZERO,
        1 => Duration::from_nanos(1),
        2 => Duration::MAX,
        3 => Duration::from_secs(31_536_000),
        4 => Duration::from_secs(3600),
        5 => Duration::new(rng.next_u64(), rng.below(1_000_000_000)),
        6 => Duration::from_nanos(rng.next_u64()),
        _ => Duration::from_millis(u64::from(rng.below(10_000))),
    }
}

impl Fields {
    /// Fields drawn from `rng`.
    fn draw(rng: &mut Rng) -> Self {
        Self {
            domain: hostile(rng),
            cause: hostile(rng),
            near: reading(rng),
            far: reading(rng),
            version: rng.next_u64(),
        }
    }
}

/// How a failure reaches the app layer; one route per arm of `BotError`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Route {
    /// Straight into the app layer.
    Kernel,
    /// Through the world layer.
    World,
    /// Through the session layer.
    Session,
    /// Through the world layer and a flow step.
    Flow,
}

impl Route {
    /// Every route, in the order `BotError` declares its arms.
    const ALL: [Self; 4] = [Self::Kernel, Self::World, Self::Session, Self::Flow];

    /// A route drawn from `rng`.
    fn draw(rng: &mut Rng) -> Self {
        match rng.below(4) {
            0 => Self::Kernel,
            1 => Self::World,
            2 => Self::Session,
            _ => Self::Flow,
        }
    }

    /// The layer name the route must end in.
    fn tag(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::World => "world",
            Self::Session => "session",
            Self::Flow => "script",
        }
    }

    /// Carry `source` up by the shipped `From` impls.
    fn wrap(self, source: KernelError) -> BotError {
        match self {
            Self::Kernel => BotError::from(source),
            Self::World => BotError::from(EcsError::from(source)),
            Self::Session => BotError::from(SessionError::from(source)),
            Self::Flow => BotError::from(FlowError::from(source)),
        }
    }
}

/// One drawn failure: its shape, its route and its fields.
struct Spec {
    /// The kernel shape.
    kind: Kind,
    /// How it reaches the app layer.
    route: Route,
    /// Its text and numbers.
    fields: Fields,
}

impl Spec {
    /// A spec drawn from `rng`.
    fn draw(rng: &mut Rng) -> Self {
        Self {
            kind: Kind::draw(rng),
            route: Route::draw(rng),
            fields: Fields::draw(rng),
        }
    }

    /// Build the failure afresh and carry it up its route.
    fn raise(&self) -> BotError {
        self.route.wrap(self.kind.build(&self.fields))
    }

    /// The line the layers must leave the kernel's own as: a flow adds only
    /// its (empty) path and a colon, and nothing else adds a word.
    fn line(&self) -> String {
        let bare = self.kind.build(&self.fields).to_string();
        match self.route {
            Route::Flow => format!(": {bare}"),
            Route::Kernel | Route::World | Route::Session => bare,
        }
    }

    /// Hold `error` to the table: one arm, the route's arm, the shape's answer,
    /// and the kernel's own line.
    fn verify(&self, error: &BotError, seed: u64, draw: u32) {
        let (certainty, class) = self.kind.answer();
        let at = format!(
            "seed {seed:#x} draw {draw} {:?}/{:?}",
            self.kind, self.route
        );
        assert_eq!(arms(error), 1, "{at}: one arm");
        assert_eq!(layer_of(error), self.route.tag(), "{at}: the route's arm");
        assert_eq!(error.dispatch_certainty(), certainty, "{at}: certainty");
        assert_eq!(error.retry_class(), class, "{at}: class");
        assert_eq!(
            error.to_string(),
            self.line(),
            "{at}: the layers add no words"
        );
    }
}

/// How many arms of `BotError` `error` is in.
fn arms(error: &BotError) -> usize {
    [
        matches!(error, BotError::Kernel(_)),
        matches!(error, BotError::World(_)),
        matches!(error, BotError::Session(_)),
        matches!(error, BotError::Script(_)),
    ]
    .into_iter()
    .filter(|hit| *hit)
    .count()
}

/// Run `sim` twice on every seed and require the same receipt.
fn replays(sim: impl Fn(u64) -> Hash) -> TestResult {
    for seed in SEEDS {
        let first = sim(seed)?;
        let again = sim(seed)?;
        assert_eq!(first, again, "seed {seed:#x} must replay");
    }
    Ok(())
}

/// What kind of error `link` is, by type and not by text.
fn tag(link: &(dyn Error + 'static)) -> &'static str {
    if link.is::<FlowError>() {
        "flow"
    } else if link.is::<EcsError>() {
        "world"
    } else if link.is::<StoreError>() {
        "store"
    } else if link.is::<io::Error>() {
        "io"
    } else if link.is::<SessionError>() {
        "session"
    } else if link.is::<KernelError>() {
        "kernel"
    } else {
        "unknown"
    }
}

/// The types `error.source()` visits, outermost first, up to [`WALK_CAP`].
fn walk(error: &BotError) -> Vec<&'static str> {
    let mut seen = Vec::new();
    let mut next = error.source();
    while let Some(link) = next {
        seen.push(tag(link));
        if seen.len() > WALK_CAP {
            break;
        }
        next = link.source();
    }
    seen
}

/// Run an infallible family twice on every seed and require the same receipt.
fn replays_total(sim: fn(u64) -> u64) -> TestResult {
    replays(|seed| Ok(sim(seed)))
}

/// A step path for a located flow failure.
fn path(label: &str) -> Arc<str> {
    Arc::from(label)
}

// ── replay ─────────────────────────────────────────────────────────────────

/// Every family, by name.
const SIMS: [(&str, Sim); 14] = [
    ("saboteur_world", sim_variants::sim_saboteur_world),
    (
        "saboteur_session_and_flow",
        sim_variants::sim_saboteur_session_and_flow,
    ),
    ("weird_bytes", |seed| Ok(sim_bytes::sim_weird_bytes(seed))),
    ("giant_and_empty", |seed| {
        Ok(sim_bytes::sim_giant_and_empty(seed))
    }),
    ("double_clicker", |seed| {
        Ok(sim_bytes::sim_double_clicker(seed))
    }),
    ("fat_finger", sim_bytes::sim_fat_finger),
    (
        "wrong_order_source_walk",
        sim_walks::sim_wrong_order_source_walk,
    ),
    (
        "wrong_order_every_route",
        sim_walks::sim_wrong_order_every_route,
    ),
    ("neighbour", sim_walks::sim_neighbour),
    (
        "liar_upstream_io_and_store",
        sim_upstream::sim_liar_upstream_io_and_store,
    ),
    ("liar_upstream_cause_text", |seed| {
        Ok(sim_upstream::sim_liar_upstream_cause_text(seed))
    }),
    ("pulled_plug", sim_upstream::sim_pulled_plug),
    ("clock_fool", |seed| Ok(sim_upstream::sim_clock_fool(seed))),
    ("monkey", |seed| Ok(sim_walks::sim_monkey(seed))),
];

#[test]
/// The divergence oracle: a family that ignored its seed would replay and still fail here.
fn sim_error_layers_every_family_hashes_its_seed_and_distinct_seeds_differ() -> TestResult {
    for entry in SIMS {
        let name = entry.0;
        let first = (entry.1)(SEEDS[0])?;
        let second = (entry.1)(SEEDS[1])?;
        assert_ne!(first, second, "{name}: two seeds drew alike");
    }
    Ok(())
}
