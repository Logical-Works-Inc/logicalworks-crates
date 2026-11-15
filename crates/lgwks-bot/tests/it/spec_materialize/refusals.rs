//! Registry and shape refusals, and the `NeedSet` helpers.

use lgwks_bot::spec::{Bot, BotSpec};
use lgwks_bot::{Admission, EcsError, GrantSet, Need, NeedSet};

use super::{DUPLICATE_DOMAINS, SPEC_DOMAINS, TestResult, scope};

#[test]
fn a_duplicate_registry_is_refused_naming_the_identifier() -> TestResult<()> {
    let spec = BotSpec::from_json(
        r#"{"version":1,"name":"dup","chains":[
            {"source":"test::counter","target":"3",
             "on":[["always",{"domain":"test::page","target":"a"}]]}]}"#,
    )?;
    let refusal = Bot::from_spec(&spec, &DUPLICATE_DOMAINS, &GrantSet::empty(), scope()?);

    match refusal {
        Err(Admission::Refused(EcsError::DuplicateDomain {
            domain,
            role,
            first,
            second,
        })) => {
            assert_eq!(domain, "test::counter");
            assert_eq!(role, "source");
            assert_eq!((first, second), (0, 1));
            Ok(())
        }
        other => Err(format!("expected a duplicate-domain refusal, got {other:?}").into()),
    }
}

#[test]
fn malformed_json_is_a_typed_error() {
    assert!(
        matches!(
            BotSpec::from_json("{ this is not json"),
            Err(EcsError::MalformedSpec { .. })
        ),
        "malformed JSON must be a typed refusal, never a panic"
    );
}

#[test]
fn unsupported_version_is_a_typed_error() {
    let refusal = BotSpec::from_json(r#"{"version":999,"name":"x","chains":[]}"#);
    assert!(
        matches!(
            refusal,
            Err(EcsError::UnsupportedSpecVersion {
                found: 999,
                supported: 1
            })
        ),
        "a version this build does not implement must be refused: {refusal:?}"
    );
}

#[test]
fn empty_chains_are_refused_by_the_materializer() -> TestResult<()> {
    let spec = BotSpec::from_json(r#"{"version":1,"name":"empty","chains":[]}"#)?;
    let refusal = Bot::from_spec(&spec, &SPEC_DOMAINS, &GrantSet::empty(), scope()?);
    assert!(
        matches!(
            refusal,
            Err(Admission::Refused(EcsError::IncompleteSpec {
                field: "chains",
                ..
            }))
        ),
        "a materialized bot with no chains is a refused document: {refusal:?}"
    );
    Ok(())
}

#[test]
fn need_set_helpers_are_total() {
    let empty = NeedSet::new(Vec::new());
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
    let one = NeedSet::new(vec![Need::UnknownSource {
        chain: 2,
        domain: "x".into(),
    }]);
    assert_eq!(
        one.needs()[0].chain(),
        Some(2),
        "a document need names the chain it came from"
    );
    assert_eq!(one.needs()[0].action(), None);
}
