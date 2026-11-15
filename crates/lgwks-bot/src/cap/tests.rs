//! Tests for the capability token and the shortfall it reports. Split from `cap.rs` (#990).

#[cfg(test)]
use super::{Auth, Cap, Deficit, Demand, assert_repair_closes};
use crate::error::{DispatchCertainty, KernelError};
use crate::gate::GrantSet;

/// A typed test failure, in the shape the other modules' tests use. Not a
/// `panic!`, which this workspace forbids outright.
fn failed(cause: impl Into<String>) -> KernelError {
    KernelError::DomainError {
        domain: "cap::tests".into(),
        certainty: DispatchCertainty::NotDelivered,
        cause: cause.into(),
    }
}

/// Capabilities by name, in the order given.
///
/// `&'static str` rather than `&str` because that is what `Cap::new`
/// accepts, and the restriction is the point of the `Cow`: a capability
/// name is a constant or a value the caller owns, never a borrow of
/// something shorter-lived that a proof would then outlive.
fn caps(names: &[&'static str]) -> Vec<Cap> {
    names.iter().map(|name| Cap::new(*name)).collect()
}

/// The names a deficit reports, in order — what a caller actually reads.
fn names(deficit: &Deficit) -> Vec<&str> {
    deficit
        .shortages()
        .map(|shortage| shortage.required().as_str())
        .collect()
}

/// The whole reason `Deficit` exists.
///
/// The check this replaced returned at the first uncovered capability, so
/// a caller granting what they were told was refused again for the next
/// word. Three requirements, one covered, must produce **two** shortages
/// from **one** refusal.
#[test]
fn a_check_names_every_uncovered_requirement_at_once() -> Result<(), KernelError> {
    let proof = Auth::new(caps(&[Cap::NET]));
    let required = caps(&[Cap::NET, Cap::FS, Cap::SYS]);

    match proof.check(&required) {
        Err(KernelError::CapabilityDenied { deficit }) => {
            assert_eq!(
                names(&deficit),
                vec![Cap::FS, Cap::SYS],
                "the deficit must carry every uncovered requirement, in declaration order: \
                 {deficit}"
            );
            assert_eq!(deficit.len(), 2);
            assert_eq!(
                deficit.first().required(),
                &Cap::fs(),
                "first is the first one, not an arbitrary one: {deficit}"
            );
            Ok(())
        }
        other => Err(failed(format!(
            "a proof covering `bot.net` must not admit `bot.fs` or `bot.sys`: {other:?}"
        ))),
    }
}

#[test]
fn a_requirement_declared_twice_is_reported_once() -> Result<(), KernelError> {
    let proof = Auth::new(Vec::new());

    match proof.check(&caps(&[Cap::NET, Cap::NET, Cap::FS])) {
        Err(KernelError::CapabilityDenied { deficit }) => {
            assert_eq!(
                names(&deficit),
                vec![Cap::NET, Cap::FS],
                "a domain listing one capability twice states one requirement: {deficit}"
            );
            assert_eq!(deficit.len(), 2, "and `len` must count requirements");
            Ok(())
        }
        other => Err(failed(format!("an empty proof covers nothing: {other:?}"))),
    }
}

/// `to_grant_set` is the "seek the missing pieces" step: the repair is
/// derived from the diagnostic rather than transcribed from it, which is
/// where a hand-written repair drops everything after the first line.
#[test]
fn the_deficit_derives_the_grant_set_that_repairs_it() -> Result<(), KernelError> {
    let proof = Auth::new(caps(&[Cap::NET]));
    let required = caps(&[Cap::NET, Cap::FS, Cap::SYS]);

    match proof.check(&required) {
        Err(KernelError::CapabilityDenied { deficit }) => {
            let held = GrantSet::empty().grant(Cap::net());
            let closed = assert_repair_closes(&deficit, &held, &required);
            assert!(
                closed
                    .grant(Cap::net())
                    .uncovered(&required, &Demand::new("test::domain"))
                    .is_empty(),
                "re-granting a capability the set already holds changes nothing"
            );
            Ok(())
        }
        other => Err(failed(format!(
            "expected a denial for two uncovered capabilities: {other:?}"
        ))),
    }
}

/// The invariant `covers_cap`'s binary search rests on.
///
/// A proof is a set the caller happened to pass as a `Vec`; sorting and
/// de-duplicating at mint is what makes each coverage question logarithmic
/// rather than a scan, and the scan was the quadratic term the benchmark
/// measured.
#[test]
fn a_proof_holds_its_capabilities_sorted_and_deduplicated() {
    let proof = Auth::new(caps(&[Cap::SYS, Cap::NET, Cap::SYS, Cap::FS]));

    assert_eq!(
        proof.covers(),
        caps(&[Cap::FS, Cap::NET, Cap::SYS]).as_slice(),
        "a proof is a set: name-ordered, each capability once"
    );
    assert!(
        proof.covers_cap(&Cap::net()),
        "membership answers for a held cap"
    );
    assert!(
        !proof.covers_cap(&Cap::notify()),
        "and denies one it does not hold"
    );
}

/// `Auth::uncovered` and `GrantSet::uncovered` are the same walk, which is
/// what keeps a proof and a gate from naming different holes for the same
/// requirement. The only intended difference is attribution.
#[test]
fn the_proof_and_the_gate_name_the_same_shortfall() {
    let required = caps(&[Cap::NET, Cap::FS]);
    let proof = Auth::new(Vec::new());
    let gate = GrantSet::empty();

    let from_proof = proof.uncovered(&required);
    let from_gate = gate.uncovered(&required, &Demand::new("test::domain"));

    let proof_names: Vec<&str> = from_proof
        .iter()
        .map(|shortage| shortage.required().as_str())
        .collect();
    let gate_names: Vec<&str> = from_gate
        .iter()
        .map(|shortage| shortage.required().as_str())
        .collect();
    assert_eq!(
        proof_names, gate_names,
        "the two checks must agree on which requirements are unmet"
    );

    assert!(
        from_proof
            .iter()
            .all(|shortage| shortage.demand().is_none()),
        "a bare check is handed a capability list and no caller, so it names no domain"
    );
    assert_eq!(
        from_gate
            .first()
            .and_then(|shortage| shortage.demand())
            .map(Demand::domain),
        Some("test::domain"),
        "and the gate, which was told one, must carry it"
    );
}

/// The invariant `Cap`'s `Cow` rests on, and the reason it is safe to make
/// the shipped constants borrow their names.
///
/// A capability that arrived from JSON is `Owned`; a capability built from
/// a constant is `Borrowed`. If those compared as different values, a grant
/// set built from a spec would deny a call made with a constant — the two
/// spellings of `bot.net` would be two capabilities, and the gate would
/// refuse a bot it had in fact granted. Equality, ordering and hashing
/// compare the contents, so they are one capability.
#[test]
fn a_borrowed_and_an_owned_spelling_are_one_capability() {
    let borrowed = Cap::net();
    let owned = Cap::new(String::from(Cap::NET));

    assert!(matches!(borrowed.0, std::borrow::Cow::Borrowed(_)));
    assert!(matches!(owned.0, std::borrow::Cow::Owned(_)));

    assert_eq!(borrowed, owned, "the two spellings name one capability");
    assert_eq!(Cap::net().cmp(&owned), std::cmp::Ordering::Equal);
    assert!(
        GrantSet::empty().grant(borrowed).grants(&owned),
        "a grant made with a constant must cover a capability read from a spec"
    );
    assert!(
        GrantSet::empty().grant(owned).grants(&Cap::net()),
        "and the reverse, or the gate would depend on which side did the naming"
    );
}

#[test]
fn a_covered_requirement_is_admitted() -> Result<(), KernelError> {
    let proof = Auth::new(caps(&[Cap::NET, Cap::FS]));
    proof.check(&caps(&[Cap::FS, Cap::NET]))?;
    assert!(
        proof.uncovered(&caps(&[Cap::NET, Cap::FS])).is_empty(),
        "the total form and the Result form must agree on the empty case"
    );
    Ok(())
}
