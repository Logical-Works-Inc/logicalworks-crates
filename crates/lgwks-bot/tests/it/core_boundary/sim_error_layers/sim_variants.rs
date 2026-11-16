//! The saboteur: every variant a layer raises, sent up the real chain, must
//! land in exactly one arm of `BotError`, keep its name, and answer as the
//! table says. The tables below are the hand-written ones; a variant added to a
//! layer without a row here is a variant this sweep does not yet cover.

use std::time::Duration;

use lgwks_bot::app_error::BotError;
use lgwks_bot::ecs::EcsError;
use lgwks_bot::error::{DispatchCertainty, RetryClass};
use lgwks_bot::script::FlowError;
use lgwks_bot::session::{JournalError, SessionError};

use super::{DRAWS, Hash, TestResult, arms, hostile, layer_of, path, reading, refuse, replays};
use crate::sim::{Rng, Trace};

/// One row a variant table yields: the error in its arm, the variant's name,
/// and whether the table says a plain retry is safe.
type Row = (BotError, &'static str, bool);

/// What a table builds a row over: text, a number and a clock reading.
struct Over<'a> {
    /// Free text for the variant's string fields.
    text: &'a str,
    /// A count or an index.
    number: u16,
    /// A clock reading.
    clock: Duration,
}

/// A variant table: row `row` built over `over`, or `None` past the table.
type Table = fn(u32, &Over<'_>) -> Option<Row>;

/// The world-layer variant `row`, or `None` past the table.
fn world(row: u32, over: &Over<'_>) -> Option<Row> {
    let owned = String::from(over.text);
    let (count, clock) = (usize::from(over.number), over.clock);
    let (error, name, retry) = match row {
        0 => (
            EcsError::IncompleteSpec {
                field: "name",
                cause: owned,
            },
            "IncompleteSpec",
            false,
        ),
        1 => (
            EcsError::UnregisteredDomain { domain: owned },
            "UnregisteredDomain",
            false,
        ),
        2 => (
            EcsError::DuplicateDomain {
                domain: owned,
                role: "source",
                first: 0,
                second: count,
            },
            "DuplicateDomain",
            false,
        ),
        3 => (
            EcsError::SpecTooLarge {
                bytes: count,
                limit: 1024,
            },
            "SpecTooLarge",
            false,
        ),
        4 => (
            EcsError::MalformedSpec { cause: owned },
            "MalformedSpec",
            false,
        ),
        5 => (
            EcsError::UnsupportedSpecVersion {
                found: u32::from(over.number),
                supported: 1,
            },
            "UnsupportedSpecVersion",
            false,
        ),
        6 => (
            EcsError::UnknownCondition {
                condition: owned.clone(),
                argument_parse: owned,
            },
            "UnknownCondition",
            false,
        ),
        7 => (EcsError::TickInsideRuntime, "TickInsideRuntime", false),
        8 => (
            EcsError::PollStalled {
                chain: count,
                deadline: clock,
            },
            "PollStalled",
            true,
        ),
        9 => (
            EcsError::PollDeadlineUnbounded { deadline: clock },
            "PollDeadlineUnbounded",
            false,
        ),
        10 => (
            EcsError::PollDeadlineExceeded {
                deadline: clock,
                ceiling: clock,
            },
            "PollDeadlineExceeded",
            false,
        ),
        11 => (
            EcsError::EvaluateError { cause: owned },
            "EvaluateError",
            false,
        ),
        12 => (
            EcsError::TypeMismatch {
                site: "saboteur",
                chain: Some(count),
                expected: "u32",
                observed: "String",
            },
            "TypeMismatch",
            false,
        ),
        _ => return None,
    };
    Some((BotError::from(error), name, retry))
}

/// The session-layer variant `row`, or `None` past the table.
fn session(row: u32, over: &Over<'_>) -> Option<Row> {
    let owned = String::from(over.text);
    let count = usize::from(over.number);
    let (error, name) = match row {
        0 => (
            SessionError::FlowTooLarge {
                bytes: count,
                limit: 64,
            },
            "FlowTooLarge",
        ),
        1 => (
            SessionError::MalformedFlow { cause: owned },
            "MalformedFlow",
        ),
        2 => (
            SessionError::InvalidTransitionTarget {
                from: owned.clone(),
                target: owned,
            },
            "InvalidTransitionTarget",
        ),
        3 => (
            SessionError::MissingAskRoute {
                node: owned.clone(),
                option: owned,
            },
            "MissingAskRoute",
        ),
        4 => (
            SessionError::VariableNeverWritten { name: owned },
            "VariableNeverWritten",
        ),
        5 => (
            SessionError::UnreachableNode { node: owned },
            "UnreachableNode",
        ),
        6 => (SessionError::SessionTerminated, "SessionTerminated"),
        7 => (
            SessionError::SessionNotAwaitingAnswer,
            "SessionNotAwaitingAnswer",
        ),
        8 => (SessionError::PredicateTypeMismatch, "PredicateTypeMismatch"),
        9 => (
            SessionError::SessionBudgetExceeded {
                steps: count,
                budget: 8,
            },
            "SessionBudgetExceeded",
        ),
        10 => (
            SessionError::ReceiptNotRecorded {
                node: owned,
                cause: JournalError::new("receipts", "closed"),
            },
            "ReceiptNotRecorded",
        ),
        _ => return None,
    };
    Some((BotError::from(error), name, false))
}

/// The script-layer variant `row`, or `None` past the table.
fn flow(row: u32, over: &Over<'_>) -> Option<Row> {
    let owned = String::from(over.text);
    let at = path(&format!("sync/each:page#{}", over.number));
    let hex = format!("{:04x}", over.number);
    let (error, name, retry) = match row {
        0 => (FlowError::Cancelled { at }, "Cancelled", false),
        1 => (
            FlowError::TimedOut {
                at,
                after: over.clock,
            },
            "TimedOut",
            true,
        ),
        2 => (FlowError::Failed { at, reason: owned }, "Failed", false),
        3 => (
            FlowError::Transient { at, reason: owned },
            "Transient",
            true,
        ),
        4 => (
            FlowError::TooDeep {
                at,
                limit: over.number,
            },
            "TooDeep",
            false,
        ),
        5 => (
            FlowError::InvalidTenant { reason: "empty" },
            "InvalidTenant",
            false,
        ),
        6 => (
            FlowError::InvalidName { reason: "too long" },
            "InvalidName",
            false,
        ),
        7 => (
            FlowError::InvalidRequestKey { reason: "nul" },
            "InvalidRequestKey",
            false,
        ),
        8 => (
            FlowError::InvalidBusinessKey { reason: "empty" },
            "InvalidBusinessKey",
            false,
        ),
        9 => (
            FlowError::InvalidBound {
                what: "fan_out",
                value: u64::from(over.number),
                max: 64,
            },
            "InvalidBound",
            false,
        ),
        10 => (FlowError::Graph { at, reason: owned }, "Graph", false),
        11 => (
            FlowError::Justified {
                at,
                rule: owned.clone(),
                reason: owned,
            },
            "Justified",
            false,
        ),
        12 => (
            FlowError::ApprovalDenied { at, id: hex },
            "ApprovalDenied",
            false,
        ),
        13 => (
            FlowError::ApprovalExpired { at, id: hex },
            "ApprovalExpired",
            false,
        ),
        14 => (
            FlowError::NeedsApproval { at, want: hex },
            "NeedsApproval",
            false,
        ),
        15 => (
            FlowError::Parked {
                at,
                id: hex.clone(),
                digest: hex,
                expires_at: u64::from(over.number),
            },
            "Parked",
            false,
        ),
        _ => return None,
    };
    Some((BotError::from(error), name, retry))
}

/// One variant table, swept by seed: every row lands in `layer`'s arm alone,
/// keeps its name under `arm`, and answers as the table says.
fn sweep(seed: u64, table: Table, layer: &str, arm: &str) -> Hash {
    let mut rng = Rng::new(seed);
    let mut trace = Trace::new();
    let mut rows = 0_u32;
    let empty = Over {
        text: "",
        number: 0,
        clock: Duration::ZERO,
    };
    while table(rows, &empty).is_some() {
        rows = rows.saturating_add(1);
    }
    assert!(
        rows >= 11,
        "{arm}: the table names the whole layer, not {rows}"
    );
    for round in 0..DRAWS {
        for row in 0..rows {
            let text = hostile(&mut rng);
            let number = u16::try_from(rng.below(1 << 16))?;
            let clock = reading(&mut rng);
            let over = Over {
                text: &text,
                number,
                clock,
            };
            let Some((error, name, retry)) = table(row, &over) else {
                return refuse(format!("{arm}: row {row} vanished"));
            };
            let at = format!("seed {seed:#x} round {round} {arm}::{name}");
            let (certainty, class) = if retry {
                (DispatchCertainty::NotDelivered, RetryClass::Safe)
            } else {
                (DispatchCertainty::Refused, RetryClass::Never)
            };
            assert_eq!(arms(&error), 1, "{at}: one arm");
            assert_eq!(layer_of(&error), layer, "{at}: arm");
            let debug = format!("{error:?}");
            assert!(
                debug.starts_with(&format!("{arm}({name}")),
                "{at}: name kept"
            );
            assert_eq!(error.dispatch_certainty(), certainty, "{at}: certainty");
            assert_eq!(error.retry_class(), class, "{at}: class");
            let line = error.to_string();
            assert!(
                !line.chars().any(char::is_control),
                "{at}: one escaped line"
            );
            trace.record(name);
            trace.record(&line);
        }
    }
    Ok(trace.hash())
}

/// Every world variant lands in `BotError::World` and nowhere else.
pub(super) fn sim_saboteur_world(seed: u64) -> Hash {
    sweep(seed, world, "world", "World")
}

/// Every session and script variant lands in its own arm and nowhere else.
pub(super) fn sim_saboteur_session_and_flow(seed: u64) -> Hash {
    let first = sweep(seed, session, "session", "Session")?;
    let second = sweep(seed ^ 1, flow, "script", "Script")?;
    Ok(first ^ second.rotate_left(1))
}

#[test]
/// The saboteur sends every world-layer variant up and finds none in two arms.
fn sim_error_layers_saboteur_every_world_variant_maps_to_one_arm() -> TestResult {
    replays(sim_saboteur_world)
}

#[test]
/// The saboteur sends every session and script variant up and finds none astray.
fn sim_error_layers_saboteur_every_session_and_flow_variant_maps_to_one_arm() -> TestResult {
    replays(sim_saboteur_session_and_flow)
}
