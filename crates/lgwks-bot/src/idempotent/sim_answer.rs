//! Fools against what a keyed effect's answer is allowed to claim (#910 defect 3).
//!
//! The ledger writes the adapter's answer down as a fact: `Applied`, `NotApplied`
//! or nothing. So the oracle here is a reference model written apart from the
//! adapter's table, by class first, and every status a `u16` can spell and every
//! transport stage and kind are checked against it. A 5xx read as "not
//! delivered", a timeout after the request left read as "not sent", or a 409
//! read as a plain refusal is the bug each family catches.
//!
//! Sources: RFC 9110 sections 15.5 and 15.6 (4xx is the client's fault, 5xx the
//! server's, and neither says whether a side effect ran), RFC 9110 15.5.9 and
//! RFC 6585 4 (408 and 429 are answered without processing the request),
//! RFC 8470 5.2 (425 Too Early), and draft-ietf-httpapi-idempotency-key-header
//! section 2.7 (409 while the key's first request is still in flight).

use lgwks_std::http::{Error, FailureKind, FailureStage};

use super::{Answer, answer_of, transport_failure};
use crate::error::{BotError, DispatchCertainty};
use crate::sim_stream::{RECEIPT_START, Stream, fold};

/// Every stage this build names.
const STAGES: [FailureStage; 11] = [
    FailureStage::Resolve,
    FailureStage::Connect,
    FailureStage::Tls,
    FailureStage::Send,
    FailureStage::Headers,
    FailureStage::Body,
    FailureStage::EofProbe,
    FailureStage::Redirect,
    FailureStage::TextDecode,
    FailureStage::Deadline,
    FailureStage::Request,
];

/// Every kind this build names.
const KINDS: [FailureKind; 7] = [
    FailureKind::Timeout,
    FailureKind::Transport,
    FailureKind::Interrupted,
    FailureKind::InvalidUtf8,
    FailureKind::RedirectLimit,
    FailureKind::InvalidRequest,
    FailureKind::Resource,
];

/// What the ledger may record for an answer, in the ledger's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ledger {
    /// The effect ran.
    Applied,
    /// The effect did not run, and a retry is a retry.
    NotApplied,
    /// Nothing: the effect may have run.
    Unsettled,
    /// The credential repair: nothing ran, the grant is wrong.
    Repair,
}

/// The reference reading of a status, by class first and then by the few
/// codes that say more than their class.
fn model(status: u16) -> Ledger {
    let class = status.div_euclid(100);
    let credential = matches!(status, 401 | 403 | 404);
    if class == 2 {
        Ledger::Applied
    } else if class == 4 && credential {
        Ledger::Repair
    } else if class == 4 && status != 409 {
        Ledger::NotApplied
    } else {
        Ledger::Unsettled
    }
}

/// The ledger reading of the adapter's answer.
fn ledger_of(answer: Answer) -> Ledger {
    match answer {
        Answer::Applied => Ledger::Applied,
        Answer::Credential => Ledger::Repair,
        Answer::NotProcessed | Answer::Refused => Ledger::NotApplied,
        Answer::Unknown => Ledger::Unsettled,
    }
}

/// The ledger reading of a transport error the adapter built; `None` for an
/// error a transport failure must never become.
fn ledger_of_error(error: &BotError) -> Option<Ledger> {
    match *error {
        BotError::DomainError {
            certainty: DispatchCertainty::Refused | DispatchCertainty::NotDelivered,
            ..
        } => Some(Ledger::NotApplied),
        BotError::EffectIndeterminate { .. } => Some(Ledger::Unsettled),
        _ => None,
    }
}

/// Catches a status read as more than it establishes: every `u16` is checked
/// against the model, and the 4xx split is pinned to its three readings.
#[test]
fn sim_answer_liar_upstream_every_status_claims_only_what_it_proves() {
    let mut seen = [0_u32; 4];
    for status in 0..=u16::MAX {
        let held = ledger_of(answer_of(status));
        assert_eq!(held, model(status), "status {status}");
        let slot = match held {
            Ledger::Applied => 0,
            Ledger::NotApplied => 1,
            Ledger::Unsettled => 2,
            Ledger::Repair => 3,
        };
        if let Some(count) = seen.get_mut(slot) {
            *count = count.saturating_add(1);
        }
    }
    assert_eq!(seen, [100, 96, 65_337, 3], "the class sizes");
    assert_eq!(
        answer_of(409),
        Answer::Unknown,
        "a key in flight is unsettled"
    );
    assert_eq!(answer_of(422), Answer::Refused, "a refused body is refused");
    assert_eq!(
        answer_of(425),
        Answer::NotProcessed,
        "too early is unprocessed"
    );
    assert_eq!(answer_of(503), Answer::Unknown, "a 5xx may have run");
    assert_eq!(
        answer_of(307),
        Answer::Unknown,
        "an unfollowed redirect may have run"
    );
}

/// Catches a failure after the request left read as "not sent": only a stage
/// before the first request byte is `NotDelivered`, whatever the kind.
#[test]
fn sim_answer_pulled_plug_only_a_failure_before_sending_is_not_delivered() {
    for stage in STAGES {
        for kind in KINDS {
            let error = Error::Failure {
                stage,
                kind,
                cause: None,
            };
            let held = ledger_of_error(&transport_failure("t".to_owned(), &error));
            let before_send = matches!(
                stage,
                FailureStage::Resolve | FailureStage::Connect | FailureStage::Tls
            );
            let expected = if before_send {
                Ledger::NotApplied
            } else {
                Ledger::Unsettled
            };
            assert_eq!(held, Some(expected), "{stage:?} {kind:?}");
        }
    }
    let unsent = [
        Error::InvalidUrl,
        Error::RedirectLimitTooLarge {
            requested: 200,
            maximum: 10,
        },
    ];
    for error in unsent {
        let held = transport_failure("t".to_owned(), &error);
        assert!(
            matches!(
                held,
                BotError::DomainError {
                    certainty: DispatchCertainty::Refused,
                    ..
                }
            ),
            "{error:?} sent nothing: {held:?}"
        );
    }
    let read = transport_failure("t".to_owned(), &Error::BodyTooLarge { limit: 1 });
    assert_eq!(
        ledger_of_error(&read),
        Some(Ledger::Unsettled),
        "a body arrived"
    );
}

/// The monkey: a seeded storm of statuses and failures, folded into a receipt
/// that replays, so a table keyed on draw order rather than value is caught.
#[test]
fn sim_answer_monkey_a_seeded_storm_replays() -> Result<(), String> {
    let walk = |seed: u64| -> Result<u64, String> {
        let mut stream = Stream::new(seed);
        let mut receipt = RECEIPT_START;
        for _ in 0..4096 {
            let status = u16::try_from(stream.below(1000)?).map_err(|error| error.to_string())?;
            let stage = stream.choose(&STAGES)?;
            let kind = stream.choose(&KINDS)?;
            let failure = Error::Failure {
                stage,
                kind,
                cause: None,
            };
            let held = ledger_of_error(&transport_failure("t".to_owned(), &failure))
                .ok_or_else(|| format!("seed {seed:#x}: {stage:?} {kind:?} left the vocabulary"))?;
            assert_eq!(
                ledger_of(answer_of(status)),
                model(status),
                "seed {seed:#x} {status}"
            );
            receipt = fold(receipt, format!("{status}:{held:?}").as_bytes());
        }
        Ok(receipt)
    };
    for seed in [0xA5_0001_u64, 0xA5_0002, 0xA5_FFFF] {
        assert_eq!(walk(seed)?, walk(seed)?, "seed {seed:#x} replays");
    }
    assert_ne!(walk(0xA5_0001)?, walk(0xA5_0002)?, "two seeds walked alike");
    Ok(())
}
