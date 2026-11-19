//! The shipped `store-conformance` binary's `keys` row (#1060).
//!
//! Source: issue #1060. A provider crate trusts this binary's verdict on the
//! shipped software provider as the reference its own provider must match, so
//! the binary must run every check and must say so in its exit code. The test
//! drives the built binary through its public interface, `--json`, and reads
//! only what an agent reading it would read: the lines and the exit status.

use std::path::PathBuf;
use std::process::Command;

/// A test's outcome: setup failures propagate instead of panicking.
type Outcome<T = ()> = Result<T, Box<dyn std::error::Error>>;

/// Kit seeds the binary runs, plus its concurrent-mint and foreign-tenant checks.
const KEY_LINES: usize = 16 + 2;

/// The binary under test, wherever this run placed it.
///
/// Read at run time because an archived test run moves it between shards.
fn binary() -> PathBuf {
    for name in [
        "NEXTEST_BIN_EXE_store-conformance",
        "NEXTEST_BIN_EXE_store_conformance",
    ] {
        if let Some(found) = std::env::var_os(name) {
            return PathBuf::from(found);
        }
    }
    PathBuf::from(env!("CARGO_BIN_EXE_store-conformance"))
}

/// Every keys check the binary runs passes on the shipped provider, and the
/// binary says so: exit 0, one passing JSON line per check, none failing.
#[test]
fn store_conformance_keys_row_passes_the_shipped_provider() -> Outcome {
    let output = Command::new(binary()).arg("--json").output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let keys: Vec<&str> = stdout
        .lines()
        .filter(|line| line.contains(r#""store":"keys""#))
        .collect();
    assert_eq!(keys.len(), KEY_LINES, "one keys line per check: {stdout}");
    assert!(
        keys.iter().all(|line| line.contains(r#""pass":true"#)),
        "every keys check passes on the shipped provider: {stdout}"
    );
    assert!(
        output.status.success(),
        "a conforming run exits 0, got {:?}: {stdout}",
        output.status.code()
    );
    assert!(
        stdout.lines().all(|line| line.starts_with('{')),
        "an agent reading --json gets one object per line, no prose: {stdout}"
    );
    Ok(())
}

/// A person running the binary without `--json` reads one line per check and
/// a closing verdict, and the exit code agrees with it.
#[test]
fn store_conformance_keys_row_reads_as_text_for_a_person() -> Outcome {
    let output = Command::new(binary()).output()?;
    let stdout = String::from_utf8(output.stdout)?;
    let keys = stdout
        .lines()
        .filter(|line| line.starts_with("keys: ") && line.ends_with(": pass"))
        .count();
    assert_eq!(keys, KEY_LINES, "one passing keys line per check: {stdout}");
    assert_eq!(
        stdout.lines().last(),
        Some("every store conforms"),
        "the last line is the verdict: {stdout}"
    );
    assert!(
        output.status.success(),
        "a conforming run exits 0: {stdout}"
    );
    Ok(())
}
