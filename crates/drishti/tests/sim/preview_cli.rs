//! Command-line convention sims: the bounded file reader, the failure text
//! the binaries print, and the digest helpers the conformance drivers use.

use std::path::PathBuf;

use drishti::cli::drive::{captured, frozen_broker, must, note, session, uuid, widget_owner};
use drishti::cli::{Failure, read_bounded};

use crate::motion_kit::{describe, ensure};
use crate::rng::SimRng;

/// Message the bounded reader gives for an oversize file.
const OVER: &str = "input too large";

/// A unique scratch file path for one sim step.
fn scratch(label: &str, seed: u64) -> PathBuf {
    std::env::temp_dir().join(format!(
        "drishti-sim-cli-{label}-{seed}-{}.bin",
        lgwks_std::id::Uuid::now_v7()
    ))
}

/// Write bytes, read them back bounded by `cap`, and remove the file.
fn read_with_cap(
    label: &str,
    seed: u64,
    bytes: &[u8],
    cap: u64,
) -> Result<Result<Vec<u8>, Failure>, String> {
    let path = scratch(label, seed);
    std::fs::write(&path, bytes).map_err(describe)?;
    let outcome = read_bounded(&path, cap, OVER);
    std::fs::remove_file(&path).map_err(describe)?;
    Ok(outcome)
}

/// Bug caught: a bounded reader that returns a prefix of an oversize file,
/// refuses a file exactly at the cap, or returns bytes that differ from the
/// file.
#[test]
fn sim_cli_read_bounded_matches_the_cap_exactly() -> Result<(), String> {
    for seed in 0..60_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(630_000));
        let cap = random.below(300);
        let length = match random.below(4) {
            0 => cap,
            1 => cap.saturating_add(1),
            2 => cap.saturating_sub(1),
            _ => random.below(600),
        };
        let mut bytes = Vec::new();
        for _ in 0..length {
            bytes.extend_from_slice(&random.next_u64().to_le_bytes()[..1]);
        }
        let outcome = read_with_cap("cap", seed, &bytes, cap)?;
        if length <= cap {
            ensure(
                matches!(outcome, Ok(ref read) if *read == bytes),
                &format!("seed {seed}: {length} bytes under cap {cap} came back as {outcome:?}"),
            )?;
        } else {
            ensure(
                matches!(outcome, Err(Failure::Input(ref message)) if message == OVER),
                &format!("seed {seed}: {length} bytes over cap {cap} came back as {outcome:?}"),
            )?;
        }
    }
    ensure(
        matches!(read_with_cap("zero", 0, &[], 0)?, Ok(ref read) if read.is_empty()),
        "an empty file under a zero cap is empty",
    )?;
    ensure(
        matches!(read_with_cap("zero", 1, &[7], 0)?, Err(Failure::Input(_))),
        "one byte over a zero cap is refused",
    )
}

/// Bug caught: a missing file reported as an input fault (or a success), or a
/// failure whose text loses its category, so an operator cannot tell a bad
/// invocation from a broken disk.
#[test]
fn sim_cli_failures_keep_their_category_and_text() -> Result<(), String> {
    let absent = scratch("absent", 0);
    let outcome = read_bounded(&absent, 10, OVER);
    ensure(
        matches!(outcome, Err(ref failure) if matches!(failure, Failure::Io(_))
            && failure.to_string().starts_with("I/O failure: ")),
        &format!("a missing file must be an I/O failure with its category, got {outcome:?}"),
    )?;
    for text in ["", "usage", "two words", "\u{e9}t\u{e9}", "line\nbreak"] {
        let from_str = Failure::from(text);
        let from_string = Failure::from(String::from(text));
        ensure(
            from_str.to_string() == text
                && from_string.to_string() == text
                && matches!(from_str, Failure::Input(_))
                && matches!(from_string, Failure::Input(_)),
            &format!("{text:?}: a message failure must print as itself"),
        )?;
    }
    let bad = Failure::from(
        String::from_utf8(vec![0xff])
            .err()
            .ok_or("0xff is not UTF-8")?,
    );
    ensure(
        matches!(bad, Failure::Utf8(_)) && bad.to_string().starts_with("invalid UTF-8: "),
        "a UTF-8 fault keeps its category",
    )?;
    let wide = Failure::from(u8::try_from(300_u16).err().ok_or("300 does not fit u8")?);
    ensure(
        matches!(wide, Failure::SizeOverflow(_)) && wide.to_string() == "input size overflow",
        "a size overflow keeps its category",
    )
}

/// Bug caught: digest helpers that differ between runs, hide a refusal, or
/// build a session for the wrong tenant, so the conformance drivers' digests
/// stop being a stable witness.
#[test]
fn sim_cli_drive_helpers_are_deterministic_and_refuse_loudly() -> Result<(), String> {
    for seed in 0..20_u64 {
        let mut random = SimRng::seed(seed.wrapping_add(640_000));
        let length = random.below_len(12).saturating_add(1);
        let tenant = random.text(length);
        let mut first = String::new();
        let mut second = String::new();
        for digest in [&mut first, &mut second] {
            let bound = session(digest, &tenant).map_err(describe)?;
            ensure(
                bound.tenant_id() == tenant,
                &format!("seed {seed}: the session names the wrong tenant"),
            )?;
            note(digest, "tenant", bound.tenant_id()).map_err(describe)?;
        }
        ensure(
            first == second,
            &format!("seed {seed}: the digest is not a function of the tenant"),
        )?;
        ensure(
            first.lines().count() == 2 && first.ends_with(&format!("tenant={tenant:?}\n")),
            &format!("seed {seed}: the digest is not one line per record: {first:?}"),
        )?;
    }
    let mut digest = String::new();
    ensure(
        uuid(&mut digest, "ok", "11111111-1111-1111-1111-111111111111").is_ok(),
        "a fixture identity must parse",
    )?;
    for bad in [
        "",
        "nope",
        "11111111-1111-1111-1111-11111111111",
        "g1111111-1111-1111-1111-111111111111",
    ] {
        let before = digest.clone();
        let refused = uuid(&mut digest, "bad-id", bad);
        ensure(
            matches!(refused, Err(Failure::Input(ref message)) if message.starts_with("bad-id refused: ")),
            &format!("{bad:?}: a bad identity must fail the drive with context, got {refused:?}"),
        )?;
        ensure(digest == before, "a refusal must not write the digest")?;
    }
    let passed: Result<u8, String> = Ok(7);
    ensure(
        matches!(must(&mut digest, "value", passed), Ok(7)) && digest.ends_with("value=7\n"),
        "a success is recorded and returned",
    )?;
    let failed: Result<u8, String> = Err(String::from("boom"));
    ensure(
        matches!(must(&mut digest, "step", failed),
            Err(Failure::Input(ref message)) if message.as_str() == "step refused: boom"),
        "a refusal names the step and the cause",
    )
}

/// Bug caught: a frozen broker whose identities, clock or audit trail differ
/// between two identical drives, so the surface digest is not reproducible.
#[test]
fn sim_cli_frozen_broker_registers_the_same_owner_twice() -> Result<(), String> {
    let mut texts = Vec::new();
    for _ in 0..2 {
        let (broker, events, _open) = frozen_broker();
        let mut digest = String::new();
        let bound = session(&mut digest, "tenant-d").map_err(describe)?;
        let handle = widget_owner(&mut digest, &broker, &bound).map_err(describe)?;
        ensure(
            handle.session_id() == crate::fixtures::SESSION_ID,
            "the owner must live in the fixed session",
        )?;
        texts.push(format!("{digest}|{:?}", captured(&events)));
    }
    ensure(
        texts.first() == texts.get(1) && texts.len() == 2,
        "two frozen drives must agree on digest and audit trail",
    )
}
