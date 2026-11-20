//! The receipt half: a `check` run binds its contract, subject, mode and scope,
//! in human and `--json` form, and a changed register changes the binding.

use std::error::Error;
use std::path::Path;

use lgwks_std::json::{self, Value};

use super::{
    Scratch, TestResult, argument, assert_has, at, check_from, clean, clean_register, subject,
};

/// A complete `check` receipt binds the subject root, the contract identity and
/// version, the exact metadata subject, the policy mode and the assurance scope.
#[test]
fn the_human_receipt_binds_contract_subject_and_mode() -> TestResult {
    let enforcement = check_from(&clean(), &["check"])?;
    assert_eq!(enforcement.code, Some(0), "stderr {:?}", enforcement.stderr);
    for needle in [
        "CONTRACT  fnv1a128:",
        "SUBJECT   fnv1a128:",
        "MODE      enforcement",
        "SCOPE  ",
    ] {
        assert_has(
            &enforcement.stdout,
            needle,
            &format!("the receipt must carry {needle:?}"),
        );
    }
    assert_has(
        &enforcement.stdout,
        &format!("schema={}", 3_u32),
        "the contract version is bound",
    );

    let register = clean_register();
    let diagnosis = check_from(&subject(), &["check", "--contract", argument(&register)?])?;
    assert_eq!(diagnosis.code, Some(2));
    assert_has(
        &diagnosis.stderr,
        "MODE      diagnosis",
        "an override is diagnosis, not enforcement",
    );
    Ok(())
}

/// The `--json` receipt exposes the same identities under stable keys.
#[test]
fn the_json_receipt_has_stable_identity_fields() -> TestResult {
    let enforcement = check_from(&clean(), &["check", "--json"])?;
    assert_eq!(enforcement.code, Some(0), "stderr {:?}", enforcement.stderr);
    let payload: Value = json::from_str(&enforcement.stdout)?;
    assert_eq!(*at(&payload, &["mode"])?, "enforcement");
    assert_eq!(*at(&payload, &["contract", "schema"])?, 3);
    assert_eq!(*at(&payload, &["contract", "entries"])?, 0);
    assert_eq!(*at(&payload, &["subject", "resolved"])?, true);
    assert_eq!(*at(&payload, &["subject", "edges"])?, 0);
    for path in [["contract", "digest"], ["subject", "digest"]] {
        let digest = at(&payload, &path)?.as_str();
        assert!(
            digest.is_some_and(|text| text.starts_with("fnv1a128:")),
            "{path:?} is present and stable: {digest:?}"
        );
    }
    let scope = at(&payload, &["scope"])?.as_str();
    assert!(
        scope.is_some_and(|text| text.contains("no enforcer")),
        "the assurance scope travels with the receipt"
    );

    let register = clean_register();
    let diagnosis = check_from(
        &subject(),
        &["check", "--contract", argument(&register)?, "--json"],
    )?;
    let payload: Value = json::from_str(&diagnosis.stdout)?;
    assert_eq!(*at(&payload, &["mode"])?, "diagnosis");
    assert_eq!(*at(&payload, &["subject", "resolved"])?, true);
    Ok(())
}

/// A receipt is a binding: changing the register changes its digest, and
/// changing the graph changes the subject digest.
#[test]
fn the_receipt_changes_when_its_subject_changes() -> TestResult {
    let scratch = Scratch::new("receipt-binding")?;
    let first = scratch.path().join("first.toml");
    let second = scratch.path().join("second.toml");
    std::fs::write(&first, "[policy]\nschema = 2\nenforce = true\n")?;
    std::fs::write(
        &second,
        "[policy]\nschema = 2\nenforce = true\n# a changed register\n",
    )?;

    let digest_of = |register: &Path| -> Result<String, Box<dyn Error>> {
        let outcome = check_from(
            &clean(),
            &["check", "--contract", argument(register)?, "--json"],
        )?;
        let payload: Value = json::from_str(&outcome.stdout)?;
        let digest = at(&payload, &["contract", "digest"])?
            .as_str()
            .ok_or("the receipt carries no string contract digest")?;
        Ok(digest.to_owned())
    };

    let first_digest = digest_of(&first)?;
    let second_digest = digest_of(&second)?;
    assert_ne!(
        first_digest, second_digest,
        "a changed register must change the bound contract digest"
    );
    Ok(())
}
