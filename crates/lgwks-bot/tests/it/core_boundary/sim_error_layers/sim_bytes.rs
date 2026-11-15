//! The fools that attack the bytes: weird-bytes sends controls and 64 KiB
//! names, giant-and-empty sends fields at the limit and at nothing,
//! double-clicker raises one failure again and again, and fat-finger sends old
//! versions and empty configuration. Each failure goes up the real chain and is
//! held to the table in the parent.

use lgwks_bot::error::{DispatchCertainty, KernelError, RetryClass};
use lgwks_bot::script::FlowError;
use lgwks_bot::task::StoreError;
use lgwks_bot::{BotError, EcsError, SessionError};

use super::{DRAWS, Fields, Hash, Kind, LIMIT, Route, Spec, TestResult, replays};
use crate::sim::{Rng, Trace};

/// Characters of a two-byte glyph that fill [`LIMIT`] bytes.
const HALF_LIMIT: usize = 1 << 15;

/// A name a log reader or a path joiner trips over.
fn weird_name(rng: &mut Rng) -> String {
    let mut name = String::new();
    for _ in 0..rng.between(1, 6) {
        match rng.below(10) {
            0 => name.push('\0'),
            1 => name.push('\u{202e}'),
            2 => name.push_str("\u{200d}\u{200b}\u{feff}"),
            3 => name.push_str("../../etc"),
            4 => name.push_str("%00%2e%2e"),
            5 => name.push_str(&String::from_utf8_lossy(&[
                0xc0, 0xaf, 0xff, 0xfe, 0xed, 0xa0,
            ])),
            6 => name.push_str(&i64::MIN.to_string()),
            7 => name.push_str("NaN -0 \u{1b}[2J\r\n"),
            8 => name.push_str(&"é".repeat(HALF_LIMIT)),
            _ => {}
        }
    }
    name
}

/// One failure carrying hostile names, and how many characters its line must keep.
struct Case {
    /// The failure, in its arm.
    error: BotError,
    /// The longest name it carries, which its line may not cut short.
    floor: usize,
}

/// Hostile and 64 KiB names through five layers: never a panic, never a raw
/// control character in a line, never a field cut short.
pub(super) fn sim_weird_bytes(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let owner = weird_name(&mut rng);
        let asked = weird_name(&mut rng);
        let (owned, queried) = (owner.chars().count(), asked.chars().count());
        let tenant = StoreError::ForeignTenant {
            owner: owner.clone(),
            asked: asked.clone(),
        };
        let domain = KernelError::DomainError {
            domain: owner.clone(),
            certainty: DispatchCertainty::NotDelivered,
            cause: asked.clone(),
        };
        let condition = EcsError::UnknownCondition {
            condition: owner.clone(),
            argument_parse: asked.clone(),
        };
        let malformed = SessionError::MalformedFlow { cause: asked };
        let cases = [
            Case {
                error: BotError::from(FlowError::from(tenant)),
                floor: owned.max(queried),
            },
            Case {
                error: BotError::from(FlowError::failed(&owner)),
                floor: owned,
            },
            Case {
                error: BotError::from(domain),
                floor: owned.max(queried),
            },
            Case {
                error: BotError::from(malformed),
                floor: queried,
            },
            Case {
                error: BotError::from(condition),
                floor: owned.max(queried),
            },
        ];
        let at = format!("seed {seed:#x} draw {draw}");
        for case in &cases {
            let line = case.error.to_string();
            assert!(
                !line.chars().any(char::is_control),
                "{at}: a control reached a line"
            );
            assert!(
                line.chars().count() >= case.floor,
                "{at}: a field was cut short"
            );
            assert!(!format!("{:?}", case.error).is_empty(), "{at}: Debug");
            trace.record_number("line", line.chars().count());
        }
    }
    Ok(trace.hash())
}

#[test]
/// NUL, right-to-left marks, `../`, overlong bytes, 64 KiB names: one escaped line each.
fn sim_error_layers_weird_bytes_never_panic_and_render_one_escaped_line() -> TestResult {
    replays(sim_weird_bytes)
}

/// Fields of 0, 1, the limit, limit +- 1 and ten times the limit, on every route.
pub(super) fn sim_giant_and_empty(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for size in [0, 1, LIMIT - 1, LIMIT, LIMIT + 1, 10 * LIMIT] {
        let unit = if rng.chance(500) { "q" } else { "é" };
        let mut fields = Fields::draw(&mut rng);
        fields.domain = unit.repeat(size);
        fields.cause = unit.repeat(size);
        let kind = Kind::draw(&mut rng);
        for route in Route::ALL {
            let spec = Spec {
                kind,
                route,
                fields: fields.clone(),
            };
            let error = spec.raise();
            spec.verify(&error, seed, 0);
            let kept = error.to_string().chars().count();
            if kind != Kind::Expired {
                assert!(kept >= size, "seed {seed:#x}: {size} chars cut to {kept}");
            }
            trace.record_number("size", size);
            trace.record_number("kept", kept);
        }
    }
    Ok(trace.hash())
}

#[test]
/// Empty, one, at-limit and ten-times-limit fields cross every layer whole.
fn sim_error_layers_giant_and_empty_fields_cross_every_layer_whole() -> TestResult {
    replays(sim_giant_and_empty)
}

/// Raise one spec once, twice or a hundred times, and by two routes to one chain.
pub(super) fn sim_double_clicker(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let spec = Spec::draw(&mut rng);
        let first = spec.raise();
        spec.verify(&first, seed, draw);
        let at = format!("seed {seed:#x} draw {draw}");
        let debug = format!("{first:?}");
        let line = first.to_string();
        let clicks = match rng.below(3) {
            0 => 1,
            1 => 2,
            _ => 100,
        };
        for _ in 1..clicks {
            let again = spec.raise();
            assert_eq!(format!("{again:?}"), debug, "{at}: a click changed it");
            assert_eq!(again.to_string(), line, "{at}: a click changed the line");
            assert_eq!(
                again.retry_class(),
                first.retry_class(),
                "{at}: a click changed the class"
            );
        }
        if spec.route == Route::Flow {
            let kernel = spec.kind.build(&spec.fields);
            let long = BotError::from(FlowError::from(EcsError::from(kernel)));
            assert_eq!(
                format!("{long:?}"),
                debug,
                "{at}: two spellings of one chain"
            );
        }
        // The same key with another body is another failure.
        if spec.kind != Kind::Expired {
            let mut other = spec.fields.clone();
            other.cause.push('x');
            other.domain.push('y');
            let body = Spec {
                kind: spec.kind,
                route: spec.route,
                fields: other,
            };
            assert_ne!(body.raise().to_string(), line, "{at}: two bodies collapsed");
        }
        trace.record(&line);
    }
    Ok(trace.hash())
}

#[test]
/// Converting the same failure twice, or a hundred times, gives one failure.
fn sim_error_layers_double_clicker_converting_twice_is_identical() -> TestResult {
    replays(sim_double_clicker)
}

/// A version from an older or newer build, or none at all.
fn version(rng: &mut Rng) -> u32 {
    match rng.below(4) {
        0 => 0,
        1 => 1,
        2 => u32::MAX,
        _ => rng.below(u32::MAX),
    }
}

/// Old versions, half-migrated stores and empty fields stay typed refusals.
pub(super) fn sim_fat_finger(seed: u64) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    for draw in 0..DRAWS {
        let (found, supported) = (version(&mut rng), version(&mut rng));
        let spec = BotError::from(EcsError::UnsupportedSpecVersion { found, supported });
        let stored = u8::try_from(rng.below(256))?;
        let wanted = u8::try_from(rng.below(256))?;
        let format = StoreError::FormatVersion {
            found: stored,
            expected: wanted,
        };
        let store = BotError::from(FlowError::from(format));
        let at = format!("seed {seed:#x} draw {draw}");
        for number in [found, supported] {
            let spelled = number.to_string();
            assert!(
                spec.to_string().contains(&spelled),
                "{at}: {number} missing"
            );
        }
        for number in [stored, wanted] {
            let spelled = number.to_string();
            assert!(
                store.to_string().contains(&spelled),
                "{at}: {number} missing"
            );
        }
        assert!(
            format!("{spec:?}").starts_with("World(UnsupportedSpecVersion"),
            "{at}"
        );
        assert!(
            format!("{store:?}").starts_with("Script(Store"),
            "{at}: a half-migrated store is a store fault, not a drift"
        );
        let empties = [
            BotError::from(EcsError::IncompleteSpec {
                field: "",
                cause: String::new(),
            }),
            BotError::from(SessionError::MalformedFlow {
                cause: String::new(),
            }),
            BotError::from(EcsError::UnknownCondition {
                condition: String::new(),
                argument_parse: String::new(),
            }),
            BotError::from(FlowError::failed("")),
        ];
        for error in empties.iter().chain([&spec, &store]) {
            assert_eq!(error.retry_class(), RetryClass::Never, "{at}: {error}");
            assert_eq!(
                error.dispatch_certainty(),
                DispatchCertainty::Refused,
                "{at}"
            );
            assert!(
                !error.to_string().is_empty(),
                "{at}: an empty field made an empty line"
            );
        }
        trace.record_number("found", found);
        trace.record_number("stored", stored);
    }
    Ok(trace.hash())
}

#[test]
/// An unsupported version, a half-migrated store and an empty field stay typed refusals.
fn sim_error_layers_fat_finger_versions_and_empty_fields_stay_typed_refusals() -> TestResult {
    replays(sim_fat_finger)
}
