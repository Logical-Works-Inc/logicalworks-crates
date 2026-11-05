//! The review path's wire contract, across many seeds: the pull-request
//! object the adapter decodes, the review payload it sends and the record it
//! reads back.
//!
//! Split out of `sim_review_pr.rs` by responsibility (the size law): that file
//! simulates the reconciliation's decision, this one the shapes on the wire.
//!
//! Every commit id is drawn from one run seed the OS picks, never a constant, so
//! each run reviews commits nobody chose, as a real pull request is reviewed.
//! An assertion names the run seed, and `LWC_SIM_SEED=<seed>` replays it.

// The payload types sit on `script` and `process`, as for `sim_review_pr`; the
// run seed is drawn through `lgwks_std::random`, which `ephemeral` turns on.
#![cfg(all(feature = "script", feature = "process", feature = "ephemeral"))]

use lgwks_bot::domain::gh::{CommitId, GhError, ReviewComment, ReviewPayload, ReviewRecord};
use lgwks_std::seeded::Seeded;

use super::sim_review_pr::{pick, text_hash};

/// What a simulation reports when a precondition did not hold.
type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The variable a failed run's seed is replayed through.
const SEED_VARIABLE: &str = "LWC_SIM_SEED";

/// The seed this run draws its commits from: the one `LWC_SIM_SEED` names, or
/// a fresh one from the OS.
fn run_seed() -> Result<u64, Box<dyn std::error::Error>> {
    match std::env::var(SEED_VARIABLE) {
        Ok(text) => Ok(text.trim().parse::<u64>()?),
        Err(std::env::VarError::NotPresent) => {
            Ok(u64::from_le_bytes(lgwks_std::random::bytes::<8>()?))
        }
        Err(error) => Err(error.into()),
    }
}

/// The next lowercase hex commit id the run's stream draws.
fn drawn_sha(draw: &mut Seeded) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(48);
    while text.len() < 40 {
        // `write!` into a `String` cannot fail; the result is bound so nothing
        // here discards an error by accident.
        let _written = write!(text, "{:016x}", draw.next_u64());
    }
    text.truncate(40);
    text
}

/// GitHub's pull-request object, decoded as the adapter decodes it.
///
/// The real API nests the pinned commits as `head.sha` and `base.sha` among
/// many other fields; a decoder that read flat `head_sha` fields pins nothing
/// on every real pull request. Each case draws its own commits, number and
/// field order, and a flat-shaped answer must pin nothing.
#[test]
fn pull_object_contract_r64() -> TestResult {
    let seed = run_seed()?;
    let mut draw = Seeded::from_seed(seed);
    for index in 0..64u64 {
        let head = drawn_sha(&mut draw);
        let base = drawn_sha(&mut draw);
        let number = index.saturating_add(1);
        let head_object = format!("\"head\":{{\"ref\":\"topic-{index}\",\"sha\":\"{head}\"}}");
        let base_object = format!("\"base\":{{\"ref\":\"main\",\"sha\":\"{base}\"}}");
        let fields = [
            format!("\"number\":{number}"),
            String::from("\"state\":\"open\""),
            head_object,
            base_object,
            format!("\"title\":\"change {index}\""),
        ];
        let rotate = pick(index, fields.len())?;
        let ordered: Vec<&str> = fields[rotate..]
            .iter()
            .chain(fields[..rotate].iter())
            .map(String::as_str)
            .collect();
        let answer = format!("{{{}}}", ordered.join(","));
        let snapshot = lgwks_std::json::from_str::<lgwks_bot::domain::gh::PrSnapshot>(&answer)?;
        assert_eq!(
            snapshot.number(),
            number,
            "{SEED_VARIABLE}={seed}, case {index}: {answer}"
        );
        assert_eq!(
            snapshot.head_sha(),
            head,
            "{SEED_VARIABLE}={seed}, case {index}: the head is `head.sha`"
        );
        assert_eq!(
            snapshot.base_sha(),
            base,
            "{SEED_VARIABLE}={seed}, case {index}: the base is `base.sha`"
        );

        let flat =
            format!("{{\"number\":{number},\"head_sha\":\"{head}\",\"base_sha\":\"{base}\"}}");
        let not_github = lgwks_std::json::from_str::<lgwks_bot::domain::gh::PrSnapshot>(&flat)?;
        assert!(
            not_github.head_sha().is_empty() && not_github.base_sha().is_empty(),
            "{SEED_VARIABLE}={seed}, case {index}: a flat answer is not GitHub's shape and pins nothing"
        );
    }
    Ok(())
}

/// Inline comments are carried in the payload and read back by field.
///
/// A non-empty comment list travels on the wire (where a verifier compares it),
/// and an empty one is omitted, so a payload that declares no comments is
/// byte-for-byte what it always was.
#[test]
fn review_comments_are_carried_and_omitted_r16() -> TestResult {
    let seed = run_seed()?;
    let mut draw = Seeded::from_seed(seed);
    for index in 0..16u64 {
        let subject = CommitId::new(drawn_sha(&mut draw))?;
        let comments = vec![
            ReviewComment::new(format!("src/a{index}.rs"), index.saturating_add(1), "first"),
            ReviewComment::new(
                format!("src/b{index}.rs"),
                index.saturating_add(2),
                "second",
            ),
        ];
        let payload =
            ReviewPayload::new(&subject, "COMMENT", "body", "marker")?.with_comments(comments);
        assert_eq!(
            payload.comments().len(),
            2,
            "{SEED_VARIABLE}={seed}, case {index}: both comments are carried"
        );
        let first = payload.comments().first().ok_or("the first comment")?;
        assert!(
            first.path().starts_with("src/a"),
            "{SEED_VARIABLE}={seed}, case {index}: a comment names its path"
        );
        assert_eq!(first.line(), index.saturating_add(1));
        assert_eq!(first.body(), "first");
        let wire = lgwks_std::json::to_string(&payload)?;
        assert!(
            wire.contains("\"comments\":["),
            "{SEED_VARIABLE}={seed}, case {index}: {wire}"
        );
        assert!(
            !wire.contains("\"comments\":[]"),
            "{SEED_VARIABLE}={seed}, case {index}: a non-empty list is sent, never an empty one"
        );

        let bare = ReviewPayload::new(&subject, "COMMENT", "body", "marker")?;
        assert!(
            !lgwks_std::json::to_string(&bare)?.contains("comments"),
            "{SEED_VARIABLE}={seed}, case {index}: an empty comment list is omitted from the wire"
        );
    }
    Ok(())
}

/// A review verifies in the state GitHub reports, with its marker inside the
/// body, and never under the request's spelling or another marker.
#[test]
fn review_state_and_marker_r64() -> TestResult {
    let reported = [
        ("COMMENT", "COMMENTED"),
        ("APPROVE", "APPROVED"),
        ("REQUEST_CHANGES", "CHANGES_REQUESTED"),
    ];
    let seed = run_seed()?;
    let mut draw = Seeded::from_seed(seed);
    for index in 0..64u64 {
        let subject = CommitId::new(drawn_sha(&mut draw))?;
        let (event, state) = reported[pick(index, reported.len())?];
        let body = format!("finding {index}: {}", text_hash(&index.to_string()));
        let marker = if index % 4 == 0 {
            String::new()
        } else {
            format!("tenant-{}/run-{index}", index % 5)
        };
        let payload = ReviewPayload::new(&subject, event, body.as_str(), marker.as_str())?;
        assert_eq!(
            payload.state(),
            state,
            "{SEED_VARIABLE}={seed}, case {index}: {event}"
        );
        if marker.is_empty() {
            assert_eq!(
                payload.body(),
                body,
                "{SEED_VARIABLE}={seed}, case {index}: no marker, no trailer"
            );
        } else {
            assert_eq!(
                payload.body(),
                format!("{body}\n\n<!-- lgwks-review:{marker} -->"),
                "{SEED_VARIABLE}={seed}, case {index}: the marker travels at the end of the body"
            );
        }
        let wire = lgwks_std::json::to_string(&payload)?;
        assert!(
            !wire.contains("\"marker\""),
            "{SEED_VARIABLE}={seed}, case {index}: GitHub's create request has no `marker` field: {wire}"
        );

        let read_back = ReviewRecord::new(9, subject.as_str(), state, payload.body());
        assert!(
            read_back.matches(&payload),
            "{SEED_VARIABLE}={seed}, case {index}: {event} reads back as {state}"
        );
        let echoed = ReviewRecord::new(9, subject.as_str(), event, payload.body());
        assert!(
            !echoed.matches(&payload),
            "{SEED_VARIABLE}={seed}, case {index}: `{event}` is the request's spelling, not a state GitHub reports"
        );
        let other = ReviewPayload::new(&subject, event, body.as_str(), format!("other-{index}"))?;
        assert!(
            !read_back.matches(&other),
            "{SEED_VARIABLE}={seed}, case {index}: identical text under another marker is another publication"
        );
    }
    Ok(())
}

/// A marker that could close or break out of the comment that carries it is
/// refused, wherever in the marker the hostile token is placed.
#[test]
fn marker_escape_r64() -> TestResult {
    let seed = run_seed()?;
    let subject = CommitId::new(drawn_sha(&mut Seeded::from_seed(seed)))?;
    let hostile = ["--", "-->", "<", ">", "\n", "\r", "\u{0}", "\t"];
    for index in 0..64u64 {
        let token = hostile[pick(index, hostile.len())?];
        let prefix = "p".repeat(pick(index, 9)?);
        let suffix = "s".repeat(pick(index.wrapping_mul(3), 7)?);
        let marker = format!("{prefix}{token}{suffix}");
        assert!(
            matches!(
                ReviewPayload::new(&subject, "COMMENT", "body", marker.as_str()),
                Err(GhError::Marker { .. })
            ),
            "{SEED_VARIABLE}={seed}, case {index}: {marker:?} could break out of the comment"
        );
        let length = 250_usize.saturating_add(pick(index, 13)?);
        let long = "m".repeat(length);
        let accepted = ReviewPayload::new(&subject, "COMMENT", "body", long.as_str()).is_ok();
        assert_eq!(
            accepted,
            length <= ReviewPayload::MAX_MARKER_BYTES,
            "{SEED_VARIABLE}={seed}, case {index}: a {length}-byte marker against the declared ceiling"
        );
    }
    Ok(())
}
