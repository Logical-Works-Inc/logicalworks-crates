//! Simulations of everything the driver reads back from the Restate side: the
//! workflow key, the plan file, attach answers, the worker's notices, the attempt
//! log, and the bounded wait for the server.
//!
//! Each oracle is a reference model written here, in the plainest way, from the
//! rule. The fool is whoever writes the bytes: a worker that prints half a line,
//! a server that answers an HTML error page, a log with a negative attempt.

use std::cell::Cell;
use std::collections::{BTreeSet, HashSet};
use std::io::Cursor;
use std::time::Duration;

use crate::rival::{parse_attempts, sdk_version};
use crate::rival_error::RivalError;
use crate::rival_http::{Attached, parse_attach, until};
use crate::rival_proc::{Notices, Outcome, pump};
use crate::rival_wire::{key_of, parse_key, parse_plan};
use crate::sim_kit::{Dice, TestResult};

/// A refusal as a message.
fn text_of(error: RivalError) -> String {
    format!("{error}")
}

/// A key reads back to what made it, over seeded tenants (dashes and digits in
/// them included) and the ends of the run-number range.
#[test]
fn sim_keys_read_back_to_their_tenant_and_run() -> TestResult {
    let mut dice = Dice::new(5);
    let parts = ["alpha", "beta", "a-b", "x-1", "7", "é", "-"];
    for _ in 0..500 {
        let tenant = *parts.get(dice.pick(parts.len())?).ok_or("draw")?;
        let ordinal = match dice.pick(4)? {
            0 => 0,
            1 => u32::MAX,
            _ => dice.small(u32::MAX)?,
        };
        let key = key_of(tenant, ordinal);
        assert_eq!(parse_key(&key), Some((tenant, ordinal)), "{key}");
    }
    Ok(())
}

/// Anything but the one canonical spelling is not a key.
#[test]
fn sim_hostile_keys_are_not_keys() {
    for key in [
        "",
        "-",
        "alpha",
        "alpha-",
        "-5",
        "alpha-007",
        "alpha-+1",
        "alpha-4294967296",
        "alpha- 1",
        "alpha-1 ",
        "alpha-0x1",
    ] {
        assert_eq!(parse_key(key), None, "{key:?}");
    }
}

/// A plan file reads back to the plan written, and a line that is not exactly two
/// whole numbers refuses the whole file.
#[test]
fn sim_plan_files_read_back_or_refuse_whole() -> TestResult {
    let mut dice = Dice::new(11);
    for _ in 0..200 {
        let plan: Vec<(u32, u32)> = (0..dice.pick(40)?)
            .map(|_| Ok((dice.small(6)?, dice.small(6)?)))
            .collect::<Result<_, String>>()?;
        let text: String = plan.iter().map(|(a, b)| format!("{a} {b}\n")).collect();
        assert_eq!(parse_plan(&text), Some(plan.clone()));
        let crlf = text.replace('\n', "\r\n");
        assert_eq!(parse_plan(&crlf), Some(plan));
    }
    for bad in ["1", "1 2 3", "a b", "-1 2", "1  2", " 1 2", "1 2 ", "99999999999 1", "1 2\n\n3 4\n"] {
        assert_eq!(parse_plan(bad), None, "{bad:?}");
    }
    Ok(())
}

/// What the reference says an attach answer is.
#[derive(Debug, PartialEq, Eq)]
enum Class {
    /// A finished run with this value.
    Done(u64),
    /// The run's own terminal failure.
    Failed,
    /// An error that is not the run's.
    Status,
    /// An answer that does not read.
    Malformed,
}

/// One attach answer drawn from the kinds a server can send, and what it is.
fn attach_case(dice: &mut Dice) -> Result<(u16, Vec<u8>, Class), String> {
    let value = dice.roll(1 << 40)?;
    let invocation = |code: u16, source: &str| {
        format!("{{\"code\":{code},\"message\":\"boom\",\"source\":\"{source}\"}}").into_bytes()
    };
    Ok(match dice.pick(12)? {
        0 => (200, format!("{value}\n").into_bytes(), Class::Done(value)),
        1 => (200, format!(" {value} ").into_bytes(), Class::Done(value)),
        2 => (200, b"not a number".to_vec(), Class::Malformed),
        3 => (200, vec![0xff, 0xfe, b'1'], Class::Malformed),
        4 => (500, invocation(500, "invocation"), Class::Failed),
        5 => (500, invocation(400, "invocation"), Class::Status),
        6 => (500, invocation(500, "ingress"), Class::Status),
        7 => (404, invocation(404, "invocation"), Class::Status),
        8 => (502, b"<html>bad gateway</html>".to_vec(), Class::Malformed),
        9 => (302, Vec::new(), Class::Malformed),
        10 => (500, b"{\"code\":500,\"source\":\"invocation\"}".to_vec(), Class::Malformed),
        _ => (200, format!("-{value}").into_bytes(), Class::Malformed),
    })
}

/// Attach answers are told apart as the reference says, over thousands of draws.
#[test]
fn sim_attach_answers_are_classed_like_the_reference() -> TestResult {
    let mut dice = Dice::new(21);
    for _ in 0..3_000 {
        let (status, body, want) = attach_case(&mut dice)?;
        let got = parse_attach(status, &body);
        let class = match &got {
            Ok(Attached::Done(value)) => Class::Done(*value),
            Ok(Attached::Failed { .. }) => Class::Failed,
            Err(RivalError::Status { .. }) => Class::Status,
            Err(RivalError::AttachMalformed { .. }) => Class::Malformed,
            Err(other) => return Err(format!("an unexpected refusal: {other}")),
        };
        assert_eq!(class, want, "{status} {:?}", String::from_utf8_lossy(&body));
        if let Err(RivalError::AttachMalformed { body: kept, .. }) = &got {
            assert!(kept.chars().count() <= 200);
        }
    }
    let giant = vec![b'x'; 1 << 20];
    let kept = match parse_attach(502, &giant) {
        Err(RivalError::AttachMalformed { body, .. }) => body.len(),
        other => return Err(format!("a giant body gave {other:?}")),
    };
    assert_eq!(kept, 200);
    Ok(())
}

/// The reference for the notice stream: lines as bytes, and what a reader that
/// stops at the first line it cannot read would have heard by then.
struct Heard {
    /// Keys heard parked.
    parked: HashSet<String>,
    /// Keys heard lost.
    lost: HashSet<String>,
}

/// Build a notice stream from seeded lines and the model of what is heard.
fn notice_stream(dice: &mut Dice, keys: &[String]) -> Result<(Vec<u8>, Heard), String> {
    let mut bytes = Vec::new();
    let mut heard = Heard {
        parked: HashSet::new(),
        lost: HashSet::new(),
    };
    for _ in 0..dice.pick(30)? {
        let key = keys.get(dice.pick(keys.len())?).ok_or("draw")?;
        let end = if dice.pick(2)? == 0 { "\r\n" } else { "\n" };
        match dice.pick(8)? {
            0..=2 => {
                bytes.extend(format!("parked {key}{end}").bytes());
                heard.parked.insert(key.clone());
            }
            3 | 4 => {
                bytes.extend(format!("lost {key}{end}").bytes());
                heard.lost.insert(key.clone());
            }
            5 => bytes.extend(format!("parked {key}-x extra{end}").bytes()),
            6 => bytes.extend(format!("PARKED {key}{end}").bytes()),
            _ => bytes.extend(format!("parked alpha-007{end}").bytes()),
        }
    }
    match dice.pick(4)? {
        0 => bytes.extend(b"parked alpha-1"),
        1 => bytes.extend(vec![b'z'; 300]),
        _ => {}
    }
    Ok((bytes, heard))
}

/// The notice reader hears exactly what the reference model says, however the
/// stream is dressed, torn or flooded.
#[test]
fn sim_notices_are_heard_like_the_reference_model() -> TestResult {
    let keys: Vec<String> = (0..6).map(|n| key_of(if n % 2 == 0 { "alpha" } else { "beta" }, n)).collect();
    let mut dice = Dice::new(31);
    for round in 0..600 {
        let (bytes, heard) = notice_stream(&mut dice, &keys)?;
        let notices = Notices::default();
        pump(Cursor::new(bytes.clone()), &notices).map_err(text_of)?;
        for key in &keys {
            let want = if heard.lost.contains(key) {
                Some(Outcome::Lost)
            } else if heard.parked.contains(key) {
                Some(Outcome::Parked)
            } else {
                None
            };
            let got = notices.wait_for(key, Duration::ZERO);
            match (want, got) {
                (Some(outcome), Ok(found)) => assert_eq!(outcome, found, "round {round} {key}"),
                (None, Err(RivalError::Notices { .. })) => {}
                (want, got) => {
                    return Err(format!("round {round} {key}: want {want:?}, got {got:?}\n{}", String::from_utf8_lossy(&bytes)));
                }
            }
        }
    }
    Ok(())
}

/// One key is taken once: after the news is taken, the same wait hears nothing.
#[test]
fn a_taken_notice_is_not_heard_twice() -> TestResult {
    let notices = Notices::default();
    pump(Cursor::new(b"parked alpha-4\n".to_vec()), &notices).map_err(text_of)?;
    assert_eq!(notices.wait_for("alpha-4", Duration::ZERO).map_err(text_of)?, Outcome::Parked);
    assert!(notices.wait_for("alpha-4", Duration::ZERO).is_err());
    Ok(())
}

/// The bounded wait: with a seeded script of answers it calls the probe at most
/// `tries` times, pauses between tries and not after the last, stops at the first
/// yes, and when the bound runs out returns the last cause it kept.
#[test]
fn sim_until_is_bounded_and_keeps_the_last_cause() -> TestResult {
    let mut dice = Dice::new(41);
    for round in 0..1_000 {
        let tries = 1 + dice.small(8)?;
        let script: Vec<u32> = (0..tries + 3).map(|_| dice.small(6)).collect::<Result<_, _>>()?;
        // 0 means yes, 1 an error, anything else a no.
        let calls = Cell::new(0_u32);
        let pauses = Cell::new(0_u32);
        let pause = |_gap: Duration| pauses.set(pauses.get() + 1);
        let got = until("a test door", tries, Duration::from_millis(1), &pause, || {
            let at = calls.get();
            calls.set(at + 1);
            let step = usize::try_from(at).ok().and_then(|index| script.get(index));
            match step {
                Some(0) => Ok(true),
                Some(1) => Err(RivalError::Notices { reason: "an error" }),
                _ => Ok(false),
            }
        });
        let window: Vec<u32> = script.iter().copied().take(usize::try_from(tries).map_err(|e| e.to_string())?).collect();
        let first_yes = window.iter().position(|step| *step == 0);
        match first_yes {
            Some(at) => {
                assert!(got.is_ok(), "round {round}");
                assert_eq!(calls.get() as usize, at + 1);
                assert_eq!(pauses.get() as usize, at);
            }
            None => {
                assert_eq!(calls.get(), tries, "round {round}");
                assert_eq!(pauses.get(), tries - 1);
                let kept_error = window.contains(&1);
                match got {
                    Err(RivalError::Notices { .. }) => assert!(kept_error, "round {round}"),
                    Err(RivalError::NotReady { tries: said, .. }) => {
                        assert!(!kept_error && said == tries, "round {round}");
                    }
                    other => return Err(format!("round {round}: {other:?}")),
                }
            }
        }
    }
    Ok(())
}

/// Attempt logs read back to the retries and failures a model counted, and a line
/// that contradicts itself refuses the whole log.
#[test]
fn sim_attempt_logs_count_like_the_model() -> TestResult {
    let mut dice = Dice::new(51);
    for round in 0..300 {
        let (mut log, mut retries, mut failed) = (String::new(), 0_u64, 0_u64);
        let mut seen: BTreeSet<(u32, u32)> = BTreeSet::new();
        for _ in 0..dice.pick(60)? {
            let run = dice.small(50)?;
            let attempt = 1 + dice.small(5)?;
            if !seen.insert((run, attempt)) {
                continue;
            }
            let fails = dice.pick(2)? == 0;
            if !fails && attempt < 2 {
                continue;
            }
            log.push_str(&format!("{} s3-outside {attempt} {}\n", key_of("alpha", run), if fails { "failed" } else { "ok" }));
            retries += u64::from(attempt >= 2);
            failed += u64::from(fails);
        }
        assert_eq!(parse_attempts(&log).map_err(text_of)?, (retries, failed), "round {round}");
    }
    for bad in [
        "alpha-1 s3-outside 1 ok\n",
        "alpha-1 s3-outside 0 failed\n",
        "alpha-1 s3-outside x failed\n",
        "alpha-01 s3-outside 2 ok\n",
        "alpha-1 s3-outside 2 maybe\n",
        "alpha-1 s3-outside 2\n",
        "alpha-1 s3-outside 2 ok extra\n",
        "alpha-1  2 ok\n",
    ] {
        assert!(matches!(parse_attempts(bad), Err(RivalError::WorkerRecord { .. })), "{bad:?}");
    }
    Ok(())
}

/// The SDK the row names is the one the manifest pins, in any layout that pin
/// takes, and a manifest without the pin is refused.
#[test]
fn the_sdk_version_is_the_manifest_pin() -> TestResult {
    let real = sdk_version(include_str!("../Cargo.toml")).map_err(|e| e.line())?;
    assert_eq!(real, "0.13.0");
    let spaced = "[dependencies]\n  restate-sdk = \"=1.2.3\" # gap: x\n";
    assert_eq!(sdk_version(spaced).map_err(|e| e.line())?, "1.2.3");
    let table = "[dependencies]\nrestate-sdk = { version = \"=4.5.6\", default-features = false, features = [\"x\"] } # gap\n";
    assert_eq!(sdk_version(table).map_err(|e| e.line())?, "4.5.6");
    for bad in [
        "",
        "restate-sdk = \"1.2.3\"\n",
        "tokio = \"=1\"\n",
        "restate-sdk = { version = \"1.2.3\" }\n",
        "restate-sdk = { default-features = false }\n",
        "restate-sdk-macros = \"=1.2.3\"\n",
    ] {
        assert!(sdk_version(bad).is_err(), "{bad:?}");
    }
    Ok(())
}
