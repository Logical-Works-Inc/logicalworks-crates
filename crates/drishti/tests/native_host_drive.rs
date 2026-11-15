//! Journey of the shipped `api-drive-host` binary (issues drishti#8, drishti#9, drishti#10).
//!
//! The binary is the run path that reaches the audit vocabulary, the scoped
//! inspectors, and the capped governance read. This test runs it as a user
//! would and reads what it prints: one JSON line per audited broker decision,
//! the own-scope counts, and the newest governance rows. One test runs the
//! binary, because the driver keeps a file store in the temp directory and two
//! concurrent drives would share it.

use std::process::Command;

/// Run the shipped driver once and return its standard output.
fn drive() -> Result<String, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_api_drive_host"))
        .output()
        .map_err(|error| format!("driver did not start: {error}"))?;
    output
        .status
        .success()
        .then_some(())
        .ok_or_else(|| format!("driver failed: {}", String::from_utf8_lossy(&output.stderr)))?;
    String::from_utf8(output.stdout).map_err(|error| format!("driver output not UTF-8: {error}"))
}

/// The audit JSON lines of a drive, in print order.
fn audit_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| line.starts_with("{\"seq\":"))
        .collect()
}

/// The `seq` of an audit line.
fn seq_of(line: &str) -> Result<u64, String> {
    let digits: String = line
        .trim_start_matches("{\"seq\":")
        .chars()
        .take_while(char::is_ascii_digit)
        .collect();
    digits
        .parse()
        .map_err(|error| format!("seq not a number in {line}: {error}"))
}

/// Audit lines count up one by one from zero: no decision is skipped or
/// repeated, whatever path it took.
fn check_seq_counts_up(lines: &[&str]) -> Result<(), String> {
    for (index, line) in lines.iter().enumerate() {
        let want = u64::try_from(index).map_err(|error| format!("index overflow: {error}"))?;
        (seq_of(line)? == want)
            .then_some(())
            .ok_or_else(|| format!("seq gap at {want}: {line}"))?;
    }
    Ok(())
}

/// A wide read without the grant and a command bar without the capability
/// each print the permission they lacked.
fn check_denials_name_the_permission(lines: &[&str]) -> Result<(), String> {
    let wide = lines.iter().any(|line| {
        line.contains("\"reach\":\"all_tenants\"")
            && line.contains("\"missing\":\"inspect.cross_tenant\"")
    });
    let bar = lines.iter().any(|line| {
        line.contains("\"detail\":\"command_bar\"") && line.contains("\"missing\":\"intent.emit\"")
    });
    wide.then_some(())
        .ok_or_else(|| String::from("the ungranted wide read was not audited"))?;
    bar.then_some(())
        .ok_or_else(|| String::from("the ungranted command bar was not audited"))
}

/// A printed line of the digest must equal `want` exactly.
fn check_line(output: &str, want: &str) -> Result<(), String> {
    if output.lines().any(|line| line == want) {
        return Ok(());
    }
    Err(format!("digest line missing or wrong: {want}"))
}

/// Same frozen clock and ids give the same output (a reproducible record),
/// and the output carries the ordered audit trail, the named denials, the
/// own-scope counts, and the capped governance read (drishti#8, drishti#9, drishti#10).
#[test]
fn sim_api_drive_host_journey_is_audited_and_replayable() -> Result<(), String> {
    let first = drive()?;
    let second = drive()?;
    (first == second)
        .then_some(())
        .ok_or_else(|| String::from("two drives of the frozen broker diverged"))?;
    let lines = audit_lines(&first);
    (!lines.is_empty())
        .then_some(())
        .ok_or_else(|| String::from("the drive printed no audit line"))?;
    check_seq_counts_up(&lines)?;
    check_denials_name_the_permission(&lines)?;
    check_line(&first, "scope-counts=(1, 1, InvalidHandle)")?;
    check_lifecycle(&first)?;
    check_line(&first, "recent-rows=(2, 10000, 1, Some(\"drive.pong\"))")
}

/// Every lifecycle step the driver prints, in print order: the step, the audit
/// action and detail it must have left, and the decision the broker took.
const LIFECYCLE: [(&str, &str, &str, &str); 11] = [
    ("subscribe", "register", "subscription", "allowed"),
    ("unsubscribe", "revoke", "subscription", "allowed"),
    ("register_ml_peer", "register", "ml_peer", "allowed"),
    (
        "emit_prediction",
        "route",
        "09000000-0000-0000-0000-000000000000",
        "allowed",
    ),
    ("unregister_ml_peer", "revoke", "ml_peer", "allowed"),
    (
        "emit_prediction_forged",
        "emit",
        "prediction",
        "denied:invalid handle",
    ),
    (
        "register_command_bar_denied",
        "register",
        "command_bar",
        "denied:capability not granted: intent.emit",
    ),
    ("register_command_bar", "register", "command_bar", "allowed"),
    (
        "emit_intent",
        "route",
        "0c000000-0000-0000-0000-000000000000",
        "allowed",
    ),
    ("unregister_command_bar", "revoke", "command_bar", "allowed"),
    ("unregister", "revoke", "widget:widget.drive", "allowed"),
];

/// The broker's whole lifecycle is audited step by step, in order: register,
/// subscribe, emit, unsubscribe and revoke each print the decision the broker
/// took, a forged handle after its revocation is refused, and the widget's own
/// revocation closes the drive.
fn check_lifecycle(output: &str) -> Result<(), String> {
    let mut next = 0;
    for (step, action, detail, decision) in LIFECYCLE {
        let prefix = format!("audited.{step}=");
        let (at, line) = output
            .lines()
            .enumerate()
            .find(|&(_, line)| line.starts_with(&prefix))
            .ok_or_else(|| format!("lifecycle step not printed: {step}"))?;
        (at >= next)
            .then_some(())
            .ok_or_else(|| format!("lifecycle step out of order: {step}"))?;
        next = at;
        let wants = [
            format!("\"action\":\"{action}\""),
            format!("\"detail\":\"{detail}\""),
            format!("\"decision\":\"{decision}\""),
        ];
        for want in &wants {
            line.contains(want.as_str())
                .then_some(())
                .ok_or_else(|| format!("{step} did not audit {want}: {line}"))?;
        }
    }
    Ok(())
}
