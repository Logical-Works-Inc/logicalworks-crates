//! `lgwks tls` driven through the shipped binary (ADR 0015).
//!
//! A harness operator checks from the shell that this build's agent clients
//! share rustls on ring: the human line, the `--json` object an agent reads and
//! the exit code all say so, and a stray argument is refused by name. A build
//! without `tls-ring` refuses the command and names the feature to install.

use std::error::Error;

use crate::lgwks_scratch::{Scratch, run_lgwks};

/// What this test returns.
type TestResult = Result<(), Box<dyn Error>>;

#[cfg(feature = "tls-ring")]
#[test]
fn lgwks_tls_installs_ring_and_says_so_to_a_person_and_an_agent() -> TestResult {
    let scratch = Scratch::fresh("tls")?;
    let agent = run_lgwks(scratch.dir(), &["tls", "--json"])?;
    assert_eq!(agent.code, Some(0), "stderr: {}", agent.stderr);
    assert_eq!(
        String::from_utf8(agent.stdout)?,
        "{\"provider\":\"ring\",\"installed\":true}\n"
    );
    let person = run_lgwks(scratch.dir(), &["tls"])?;
    assert_eq!(person.code, Some(0), "stderr: {}", person.stderr);
    assert_eq!(
        String::from_utf8(person.stdout)?,
        "tls: rustls on ring is this process's provider\n"
    );
    Ok(())
}

#[cfg(feature = "tls-ring")]
#[test]
fn lgwks_tls_refuses_a_stray_argument_by_name() -> TestResult {
    let scratch = Scratch::fresh("tls-stray")?;
    let outcome = run_lgwks(scratch.dir(), &["tls", "--provider=aws-lc"])?;
    assert_eq!(outcome.code, Some(2));
    assert!(outcome.stdout.is_empty(), "a refusal prints no verdict");
    assert!(
        outcome.stderr.contains("--provider=aws-lc"),
        "the refusal names the argument: {}",
        outcome.stderr
    );
    Ok(())
}

#[cfg(not(feature = "tls-ring"))]
#[test]
fn lgwks_tls_without_its_feature_names_the_feature_to_install() -> TestResult {
    let scratch = Scratch::fresh("tls-absent")?;
    let outcome = run_lgwks(scratch.dir(), &["tls", "--json"])?;
    assert_eq!(outcome.code, Some(2));
    assert!(
        outcome.stderr.contains("--features tls-ring"),
        "the refusal names the feature: {}",
        outcome.stderr
    );
    Ok(())
}
