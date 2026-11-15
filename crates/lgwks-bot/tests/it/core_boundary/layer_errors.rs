//! One typed error per layer (#990, ADR 0061): a lower layer's failure rides up
//! through each layer that wraps it by `From`, unchanged, and the app layer's
//! `BotError` names the layer that raised it.
//!
//! The in-crate tests prove the kernel, world and session wrappers where they
//! are defined; this proves the two that only the public surface reaches, the
//! script layer's `FlowError` and the app layer's `BotError`, and that a
//! failure keeps its retry class, its dispatch certainty and its rendered line
//! across every hop. The simulations draw the failure and the hop from a seed.

use lgwks_bot::error::{DispatchCertainty, KernelError, RetryClass};
use lgwks_bot::script::FlowError;
use lgwks_bot::{BotError, EcsError, SessionError};

use crate::sim::{Rng, Trace};

/// The seeds every family replays.
const SEEDS: [u64; 4] = [
    0x990_0000_0000_0001,
    0x990_0000_0000_0002,
    0x990_FFFF_0000_0003,
    0x1990_0990_0000_0004,
];

/// Failures drawn per seed.
const DRAWS: u32 = 128;

/// The domain every drawn failure names.
const DOMAIN: &str = "layer::test";

/// The shape of a drawn kernel failure. `KernelError` is not `Clone`, so one
/// draw is built once per layer it is wrapped into.
#[derive(Clone, Copy)]
enum Shape {
    /// A `DomainError` carrying this certainty.
    Domain(DispatchCertainty),
    /// An `EffectIndeterminate`.
    Indeterminate,
    /// A lost compare-and-set.
    Conflict,
}

/// The certainty draw `n` names.
fn certainty_of(n: u32) -> DispatchCertainty {
    match n {
        0 => DispatchCertainty::Unsettled,
        1 => DispatchCertainty::Occurred,
        2 => DispatchCertainty::Refused,
        _ => DispatchCertainty::NotDelivered,
    }
}

impl Shape {
    /// A shape drawn from `rng`.
    fn draw(rng: &mut Rng) -> Self {
        match rng.below(6) {
            4 => Self::Indeterminate,
            5 => Self::Conflict,
            n => Self::Domain(certainty_of(n)),
        }
    }

    /// The kernel failure this shape names, with `text` in its free fields.
    fn build(self, text: &str) -> KernelError {
        let domain = String::from(DOMAIN);
        let cause = String::from(text);
        match self {
            Self::Domain(certainty) => KernelError::DomainError {
                cause,
                domain,
                certainty,
            },
            Self::Indeterminate => KernelError::EffectIndeterminate { cause, domain },
            Self::Conflict => KernelError::Conflict {
                record: cause,
                found: 2,
                expected: 1,
                domain,
            },
        }
    }
}

/// The layer a drawn failure is raised in, and how it reaches the app layer.
#[derive(Clone, Copy)]
enum Raised {
    /// A kernel failure, wrapped straight into the app layer.
    Kernel,
    /// A kernel failure through the world layer.
    WorldCarrying,
    /// The world layer's own refusal.
    WorldOwn,
    /// A kernel failure through the session layer.
    SessionCarrying,
    /// The session layer's own refusal.
    SessionOwn,
    /// A kernel failure through the world layer into a flow.
    FlowCarrying,
    /// The script layer's own refusal.
    FlowOwn,
}

impl Raised {
    /// A layer drawn from `rng`.
    fn draw(rng: &mut Rng) -> Self {
        match rng.below(7) {
            0 => Self::Kernel,
            1 => Self::WorldCarrying,
            2 => Self::WorldOwn,
            3 => Self::SessionCarrying,
            4 => Self::SessionOwn,
            5 => Self::FlowCarrying,
            _ => Self::FlowOwn,
        }
    }

    /// The app-layer error this layer's failure becomes, and the layer tag the
    /// wrapper must carry.
    fn wrap(self, shape: Shape, text: &str) -> (BotError, &'static str) {
        match self {
            Self::Kernel => (BotError::from(shape.build(text)), "kernel"),
            Self::WorldCarrying => (BotError::from(EcsError::from(shape.build(text))), "world"),
            Self::WorldOwn => (BotError::from(EcsError::TickInsideRuntime), "world"),
            Self::SessionCarrying => (
                BotError::from(SessionError::from(shape.build(text))),
                "session",
            ),
            Self::SessionOwn => (BotError::from(SessionError::SessionTerminated), "session"),
            Self::FlowCarrying => (BotError::from(FlowError::from(shape.build(text))), "script"),
            Self::FlowOwn => (BotError::from(FlowError::failed(text)), "script"),
        }
    }

    /// The retry class the failure had before any wrapping.
    fn class(self, shape: Shape, text: &str) -> RetryClass {
        match self {
            Self::Kernel | Self::WorldCarrying | Self::SessionCarrying | Self::FlowCarrying => {
                shape.build(text).retry_class()
            }
            Self::WorldOwn | Self::SessionOwn | Self::FlowOwn => RetryClass::Never,
        }
    }
}

/// The layer a wrapper names.
pub(super) fn layer_of(error: &BotError) -> &'static str {
    match *error {
        BotError::Kernel(_) => "kernel",
        BotError::World(_) => "world",
        BotError::Session(_) => "session",
        BotError::Script(_) => "script",
        _ => "an unnamed layer",
    }
}

/// The glyph a draw names: plain and multi-byte letters, and the controls a log
/// line or a terminal acts on.
fn glyph(draw: u32) -> char {
    match draw {
        0 => 'a',
        1 => 'Z',
        2 => '7',
        3 => ' ',
        4 => 'é',
        5 => '✓',
        6 => '\n',
        7 => '\r',
        8 => '\t',
        9 => '\u{1b}',
        10 => '\u{0}',
        11 => '\u{7f}',
        _ => '\u{85}',
    }
}

/// Up to 24 drawn glyphs.
pub(super) fn hostile(rng: &mut Rng) -> String {
    let len = rng.below(25);
    (0..len).map(|_| glyph(rng.below(13))).collect()
}

/// One drawn failure: the route it takes, its kernel shape and its free text.
fn drawn(rng: &mut Rng) -> (Raised, Shape, String) {
    let raised = Raised::draw(rng);
    let shape = Shape::draw(rng);
    (raised, shape, hostile(rng))
}

#[test]
/// A kernel failure that `?` carries into the app layer is the same failure:
/// same variant inside, same class, same certainty, same line.
fn a_kernel_failure_wraps_into_the_app_error_unchanged() {
    let kernel = KernelError::EffectIndeterminate {
        domain: String::from("gh::merge"),
        cause: String::from("timed out"),
    };
    let line = kernel.to_string();
    let wrapped = BotError::from(kernel);
    assert!(
        matches!(
            wrapped,
            BotError::Kernel(KernelError::EffectIndeterminate { .. })
        ),
        "the kernel failure must ride in the Kernel arm: {wrapped}"
    );
    assert_eq!(wrapped.to_string(), line);
    assert_eq!(wrapped.retry_class(), RetryClass::RequiresEvidence);
    assert_eq!(wrapped.dispatch_certainty(), DispatchCertainty::Unsettled);
}

#[test]
/// The world and session layers wrap into their own arms, never the kernel's,
/// so a consumer matches on the layer first.
fn a_world_and_a_session_failure_wrap_into_the_app_error_by_layer() {
    let world = BotError::from(EcsError::TickInsideRuntime);
    assert!(matches!(
        world,
        BotError::World(EcsError::TickInsideRuntime)
    ));
    assert_eq!(world.retry_class(), RetryClass::Never);
    let session = BotError::from(SessionError::SessionTerminated);
    assert!(matches!(
        session,
        BotError::Session(SessionError::SessionTerminated)
    ));
    assert_eq!(session.retry_class(), RetryClass::Never);
    assert_eq!(session.dispatch_certainty(), DispatchCertainty::Refused);
}

#[test]
/// A flow failure wraps into the script arm with its location and class.
fn a_flow_failure_wraps_into_the_app_error_by_layer() {
    let flow = FlowError::transient("the peer hung up");
    let line = flow.to_string();
    let wrapped = BotError::from(flow);
    assert!(
        matches!(wrapped, BotError::Script(FlowError::Transient { .. })),
        "a flow failure must ride in the Script arm: {wrapped}"
    );
    assert_eq!(wrapped.to_string(), line);
    assert_eq!(wrapped.retry_class(), RetryClass::Safe);
    assert_eq!(
        wrapped.dispatch_certainty(),
        DispatchCertainty::NotDelivered
    );
}

#[test]
/// A kernel failure `?`-carried into a flow rides through the world layer, so
/// the flow reports what the verb reported and the class it had.
fn a_kernel_failure_reaches_a_flow_through_the_world_layer() {
    let kernel = KernelError::DomainError {
        domain: String::from("gh::pr_status"),
        certainty: DispatchCertainty::NotDelivered,
        cause: String::from("connection refused"),
    };
    let line = kernel.to_string();
    let flow = FlowError::from(kernel);
    let carried = match flow {
        FlowError::Bot { ref source, .. } => matches!(**source, EcsError::Kernel(_)),
        _ => false,
    };
    assert!(
        carried,
        "the kernel failure must ride through the world: {flow}"
    );
    assert_eq!(flow.at(), "", "unlocated until the boundary fills the path");
    assert!(
        flow.is_retryable(),
        "a not-delivered failure is a plain retry"
    );
    assert_eq!(flow.retry_class(), RetryClass::Safe);
    assert!(flow.to_string().ends_with(&line), "{flow}");
}

#[test]
/// A kernel failure keeps its retry class, its certainty and its rendered line
/// through the world layer into a flow and on into the app layer.
fn sim_a_kernel_failure_keeps_its_class_through_the_flow_and_app_layers() {
    for seed in SEEDS {
        let mut rng = Rng::new(seed);
        for draw in 0..DRAWS {
            let (_, shape, text) = drawn(&mut rng);
            let bare = shape.build(&text);
            let flow = FlowError::from(shape.build(&text));
            let flow_line = flow.to_string();
            let wrapped = BotError::from(flow);
            assert_eq!(
                wrapped.retry_class(),
                bare.retry_class(),
                "seed {seed:#x} draw {draw}"
            );
            assert_eq!(
                wrapped.dispatch_certainty(),
                BotError::from(shape.build(&text)).dispatch_certainty(),
                "seed {seed:#x} draw {draw}"
            );
            assert!(
                flow_line.ends_with(&bare.to_string()),
                "seed {seed:#x} draw {draw}: the flow line ends in the kernel's own"
            );
            assert_eq!(wrapped.to_string(), flow_line, "seed {seed:#x} draw {draw}");
        }
    }
}

#[test]
/// The app error names the layer that raised the failure, whichever of seven
/// routes it took, and the class it had before wrapping.
fn sim_the_app_error_names_the_layer_that_raised_it() {
    for seed in SEEDS {
        let mut rng = Rng::new(seed);
        for draw in 0..DRAWS {
            let (raised, shape, text) = drawn(&mut rng);
            let (wrapped, layer) = raised.wrap(shape, &text);
            assert_eq!(layer_of(&wrapped), layer, "seed {seed:#x} draw {draw}");
            assert_eq!(
                wrapped.retry_class(),
                raised.class(shape, &text),
                "seed {seed:#x} draw {draw}: the class survives the wrap"
            );
        }
    }
}

#[test]
/// Untrusted text in a failure renders as one escaped line however many layers
/// wrap it.
fn sim_a_wrapped_failure_renders_no_control_character() {
    for seed in SEEDS {
        let mut rng = Rng::new(seed);
        for draw in 0..DRAWS {
            let (raised, shape, text) = drawn(&mut rng);
            let rendered = raised.wrap(shape, &text).0.to_string();
            assert!(
                !rendered.chars().any(char::is_control),
                "seed {seed:#x} draw {draw}: {rendered:?}"
            );
        }
    }
}

/// The replay receipt of every wrap drawn under `seed`.
fn wrap_trace(seed: u64) -> u64 {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for _ in 0..DRAWS {
        let (raised, shape, text) = drawn(&mut rng);
        let (wrapped, layer) = raised.wrap(shape, &text);
        trace.record(layer);
        trace.record(&format!("{:?}", wrapped.retry_class()));
        trace.record(&wrapped.to_string());
    }
    trace.hash()
}

#[test]
/// The replay oracle and its divergence oracle: one seed, one trace, and a
/// sweep that ignored its seed would pass the first and fail the second.
fn sim_the_same_seed_wraps_the_same_way_and_distinct_seeds_differ() {
    for seed in SEEDS {
        assert_eq!(wrap_trace(seed), wrap_trace(seed), "seed {seed:#x} replays");
    }
    assert_ne!(
        wrap_trace(SEEDS[0]),
        wrap_trace(SEEDS[1]),
        "two seeds drew alike"
    );
}
