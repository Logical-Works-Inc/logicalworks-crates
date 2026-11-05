//! Port of the kernel capability-port acceptance gates
//! (`canvas-syscall/tests/port_acceptance.rs` and
//! `revocation_acceptance.rs`), re-targeted at the estate bearer tokens in
//! [`lgwks_bot::cap_token`]: offline verify, holder attenuation that never
//! widens, deterministic decisions with traces over one hierarchy, revocation
//! that collapses delegation, expiry on the injected clock, and malformed
//! input that is refused rather than panicked.

use lgwks_bot::cap_token::{
    Authority, BearerToken, Caveat, Decision, Policy, Request, RevocationList, TokenError,
};
use lgwks_bot::clock::Clock;
use std::time::Duration;

/// A test failure, in the shape the other modules' tests use.
type TestResult = Result<(), String>;

/// A deterministic stream for sweeps: no wall clock, no unseeded randomness.
struct Stream(u64);

impl Stream {
    /// The next word.
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        self.0
    }

    /// A value below `bound`, drawn from the high bits: the low bits of a
    /// congruential stream repeat on a short cycle and would reuse ids.
    fn below(&mut self, bound: u64) -> Result<u64, String> {
        let word = self.next() >> 33;
        match word.checked_rem(bound) {
            Some(value) => Ok(value),
            None => Err(String::from("a sweep bound of zero draws nothing")),
        }
    }
}

/// A virtual clock at rest.
fn clock_at(secs: u64) -> Clock {
    Clock::virtual_at(Duration::from_secs(secs))
}

/// Mint a notes read token for `/notes` with a minute of life.
fn notes_read(authority: &Authority, clock: &Clock) -> Result<BearerToken, String> {
    authority
        .mint("notes", "read", "/notes", Duration::from_secs(60), clock)
        .map_err(|cause| cause.to_string())
}

/// The policy request the tests decide.
fn read_inbox() -> Result<Request, String> {
    Request::new("notes", "read", "/notes/alice/inbox").map_err(|cause| cause.to_string())
}

/// A fresh clock and authority with a notes read token: the arrange step
/// every port test shares, so the arrange cannot drift between ports.
fn setup(secs: u64) -> Result<(Clock, Authority, BearerToken), String> {
    let clock = clock_at(secs);
    let authority = Authority::generate().map_err(|cause| cause.to_string())?;
    let token = notes_read(&authority, &clock)?;
    Ok((clock, authority, token))
}

/// Decides the inbox read under a fresh policy and empty revocations: the
/// act step every allow-path port shares.
fn decide_inbox(
    authority: &Authority,
    token: &BearerToken,
    clock: &Clock,
) -> Result<Decision, String> {
    let policy = Policy::new();
    let revocations = RevocationList::new();
    policy
        .decide(authority, token, &read_inbox()?, &revocations, clock)
        .map_err(|cause| cause.to_string())
}

/// Two decisions over the same inputs agree exactly: the one assertion the
/// determinism gates share instead of a second copy of the same six lines.
fn assert_same_decision(first: &Decision, second: &Decision) -> Result<(), String> {
    assert_eq!(
        first.inputs_hash(),
        second.inputs_hash(),
        "the same inputs replay the same inputs hash"
    );
    assert_eq!(
        first.trace_hash(),
        second.trace_hash(),
        "the same inputs replay the same trace hash"
    );
    assert_eq!(
        first.allowed(),
        second.allowed(),
        "the same inputs replay the same decision"
    );
    Ok(())
}

/// Port of `holder_can_transmit_through_a_valid_port`: a minted token verifies
/// and its request decides allow with a non-empty trace.
#[test]
fn bearer_token_verifies_and_decides_allow() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let revocations = RevocationList::new();
    authority
        .verify(&token, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    let decision = decide_inbox(&authority, &token, &clock)?;
    assert!(
        decision.allowed(),
        "a minted token covers its own request: {decision:?}"
    );
    assert!(
        !decision.trace().is_empty(),
        "every allow carries its trace: {decision:?}"
    );
    assert_eq!(
        decision.trace().first().map(|step| step.version()),
        Some(lgwks_bot::cap_token::POLICY_VERSION),
        "the trace carries the policy version: {decision:?}"
    );
    Ok(())
}

/// Port of `a_non_holder_cannot_use_the_port`: flipping one tag byte refuses
/// as tampered.
#[test]
fn tampered_token_is_refused() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let mut bytes = token.to_bytes().map_err(|cause| cause.to_string())?;
    let last = bytes
        .len()
        .checked_sub(1)
        .ok_or_else(|| String::from("a token serializes to bytes"))?;
    bytes[last] = bytes[last].wrapping_add(1);
    match BearerToken::from_bytes(bytes.as_slice()) {
        Ok(forged) => {
            let revocations = RevocationList::new();
            match authority.verify(&forged, &revocations, &clock) {
                Err(TokenError::Tampered) => Ok(()),
                other => Err(format!("a flipped tag must verify as tampered: {other:?}")),
            }
        }
        Err(TokenError::Malformed) => Ok(()),
        Err(other) => Err(format!("a flipped byte refuses typed: {other:?}")),
    }
}

/// Port of `ai_principal_cannot_use_a_port` and the shred attenuation gate: a
/// request outside the grant denies with the rule on the trace.
#[test]
fn action_exceeds_grant_denies_with_trace() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let policy = Policy::new();
    let revocations = RevocationList::new();
    let shred = Request::new("notes", "destructive", "/notes/alice/inbox")
        .map_err(|cause| cause.to_string())?;
    let decision = policy
        .decide(&authority, &token, &shred, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    assert!(
        !decision.allowed(),
        "a read-only token never permits a destructive action: {decision:?}"
    );
    assert!(
        decision.trace().iter().any(|step| step.rule() == "action"),
        "the trace names the action rule: {decision:?}"
    );
    Ok(())
}

/// Port of `cross_tenant_port_forgery_is_denied`: re-homing the audience
/// denies.
#[test]
fn cross_tenant_forgery_denied() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let policy = Policy::new();
    let revocations = RevocationList::new();
    let foreign =
        Request::new("other", "read", "/notes/alice/inbox").map_err(|cause| cause.to_string())?;
    let decision = policy
        .decide(&authority, &token, &foreign, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    assert!(
        !decision.allowed(),
        "a token minted for one audience never covers another: {decision:?}"
    );
    assert!(
        decision
            .trace()
            .iter()
            .any(|step| step.rule() == "audience"),
        "the trace names the audience rule: {decision:?}"
    );
    Ok(())
}

/// Port of `delegated_subport_transmits_within_narrowed_authority`: an
/// attenuated token still allows inside its narrowed scope.
#[test]
fn delegated_attenuated_token_allows_within_scope() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let narrowed = token
        .attenuate(&[
            Caveat::new("notes", "read", "/notes/alice", Duration::from_secs(1_030))
                .map_err(|cause| cause.to_string())?,
        ])
        .map_err(|cause| cause.to_string())?;
    let revocations = RevocationList::new();
    authority
        .verify(&narrowed, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    let decision = decide_inbox(&authority, &narrowed, &clock)?;
    assert!(
        decision.allowed(),
        "a narrowed delegation still covers its own scope: {decision:?}"
    );
    let outside =
        Request::new("notes", "read", "/notes/bob/inbox").map_err(|cause| cause.to_string())?;
    let policy = Policy::new();
    let refused = policy
        .decide(&authority, &narrowed, &outside, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    assert!(
        !refused.allowed(),
        "the same delegation stops at its own boundary: {refused:?}"
    );
    Ok(())
}

/// A widening attenuation is refused typed at the call site.
#[test]
fn widening_attenuation_is_refused_typed() -> TestResult {
    let (_, _, token) = setup(1_000)?;
    let wider = Caveat::new("notes", "read", "/other", Duration::from_secs(30))
        .map_err(|cause| cause.to_string())?;
    match token.attenuate(&[wider]) {
        Err(TokenError::WidensAuthority) => Ok(()),
        other => Err(format!("a widening attenuation must be refused: {other:?}")),
    }
}

/// Port of `revocation_absorbed_into_ledger_denies_syscall`: absorbing a token
/// id denies its verify and its decision.
#[test]
fn revocation_absorbed_denies_verify_and_decide() -> TestResult {
    let (clock, authority, token) = setup(10)?;
    let policy = Policy::new();
    let fresh = RevocationList::new();
    authority
        .verify(&token, &fresh, &clock)
        .map_err(|cause| cause.to_string())?;
    let mut revocations = RevocationList::new();
    revocations
        .revoke(token.id())
        .map_err(|cause| cause.to_string())?;
    match authority.verify(&token, &revocations, &clock) {
        Err(TokenError::Revoked) => (),
        other => return Err(format!("a revoked token must not verify: {other:?}")),
    }
    let decision = policy
        .decide(&authority, &token, &read_inbox()?, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    assert!(
        !decision.allowed(),
        "a revoked token decides deny: {decision:?}"
    );
    assert!(
        decision
            .trace()
            .iter()
            .any(|step| step.rule() == "revocation"),
        "the trace names the revocation rule: {decision:?}"
    );
    Ok(())
}

/// Port of `unconverged_device_still_allows_within_honest_window`: without the
/// revocation the same token still allows.
#[test]
fn unrevoked_token_still_allows() -> TestResult {
    let (clock, authority, token) = setup(10)?;
    let decision = decide_inbox(&authority, &token, &clock)?;
    assert!(
        decision.allowed(),
        "an unconverged revocation changes nothing: {decision:?}"
    );
    Ok(())
}

/// Port of `revoking_middle_hop_collapses_downstream_trail`: revoking the
/// delegated id collapses the delegation the holder passed on.
#[test]
fn revocation_collapses_delegated_token() -> TestResult {
    let (clock, authority, token) = setup(7)?;
    let delegated = token
        .attenuate(&[
            Caveat::new("notes", "read", "/notes/alice", Duration::from_secs(30))
                .map_err(|cause| cause.to_string())?,
        ])
        .map_err(|cause| cause.to_string())?;
    let mut revocations = RevocationList::new();
    revocations
        .revoke(delegated.id())
        .map_err(|cause| cause.to_string())?;
    match authority.verify(&delegated, &revocations, &clock) {
        Err(TokenError::Revoked) => Ok(()),
        other => Err(format!("a revoked delegation must collapse: {other:?}")),
    }
}

/// Port of `test_ttl_auto_purge`: past the expiry the token is refused with
/// the two readings.
#[test]
fn expiry_is_refused_with_typed_error() -> TestResult {
    let (clock, authority, _) = setup(100)?;
    let token = authority
        .mint("notes", "read", "/notes", Duration::from_secs(50), &clock)
        .map_err(|cause| cause.to_string())?;
    let later = clock_at(160);
    let revocations = RevocationList::new();
    match authority.verify(&token, &revocations, &later) {
        Err(TokenError::Expired { .. }) => (),
        other => return Err(format!("a lapsed token must expire typed: {other:?}")),
    }
    let decision = decide_inbox(&authority, &token, &later)?;
    assert!(
        !decision.allowed(),
        "a lapsed token decides deny: {decision:?}"
    );
    assert!(
        decision.trace().iter().any(|step| step.rule() == "expiry"),
        "the trace names the expiry rule: {decision:?}"
    );
    Ok(())
}

/// Replay after revoke is refused: the second presentation of a revoked token
/// answers revoked, never allow.
#[test]
fn replay_after_revoke_is_refused() -> TestResult {
    let (clock, authority, token) = setup(10)?;
    let mut revocations = RevocationList::new();
    let fresh = RevocationList::new();
    authority
        .verify(&token, &fresh, &clock)
        .map_err(|cause| cause.to_string())?;
    revocations
        .revoke(token.id())
        .map_err(|cause| cause.to_string())?;
    for _ in 0..3 {
        match authority.verify(&token, &revocations, &clock) {
            Err(TokenError::Revoked) => (),
            other => {
                return Err(format!(
                    "a replayed revocation must stay revoked: {other:?}"
                ));
            }
        }
    }
    Ok(())
}

/// Fuzzed malformed bytes never panic: truncations and corruptions of a valid
/// token and raw seeded bytes are all typed refusals.
#[test]
fn malformed_bytes_are_refused_never_panic() -> TestResult {
    let (_, _, token) = setup(1_000)?;
    let bytes = token.to_bytes().map_err(|cause| cause.to_string())?;
    for end in 0..bytes.len() {
        match BearerToken::from_bytes(&bytes[0..end]) {
            Ok(_) | Err(_) => (),
        }
    }
    let mut stream = Stream(0xC0FFEE);
    let token_len = bytes.len();
    let at_limit = u64::try_from(token_len).map_err(|cause| cause.to_string())?;
    for _ in 0..512 {
        let len = stream.below(96)?;
        let len_slots = usize::try_from(len).map_err(|cause| cause.to_string())?;
        let mut raw = Vec::with_capacity(len_slots);
        for _ in 0..len_slots {
            let byte = stream.below(256)?;
            let narrowed = u8::try_from(byte).map_err(|cause| cause.to_string())?;
            raw.push(narrowed);
        }
        match BearerToken::from_bytes(raw.as_slice()) {
            Ok(_) | Err(_) => (),
        }
        let pos = stream.below(at_limit)?;
        let at = usize::try_from(pos).map_err(|cause| cause.to_string())?;
        let mut cut = bytes.clone();
        if token_len != cut.len() {
            return Err(String::from("a cloned token keeps its length"));
        }
        if let Some(slot) = cut.get_mut(at) {
            *slot = slot.wrapping_add(1);
        }
        match BearerToken::from_bytes(cut.as_slice()) {
            Ok(_) | Err(_) => (),
        }
    }
    Ok(())
}

/// Same inputs and the injected clock give the same decision and the same
/// trace hash.
#[test]
fn same_inputs_same_clock_same_decision_and_trace_hash() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let first = decide_inbox(&authority, &token, &clock)?;
    let second = decide_inbox(&authority, &token, &clock)?;
    assert_same_decision(&first, &second)?;
    Ok(())
}

/// Attenuation monotonicity over seeded narrowings: whatever the narrowed
/// token allows, the opening token allows too, and the narrowed token never
/// allows more.
#[test]
fn attenuation_monotonicity_over_seeded_narrowings() -> TestResult {
    let mut stream = Stream(0xB07E5);
    for _ in 0..64 {
        let (clock, authority, token) = setup(1_000)?;
        let pick = stream.below(3)?;
        let scope = match pick {
            0 => "/notes/alice",
            1 => "/notes/alice/inbox",
            _ => "/notes",
        };
        let extra = Caveat::new("notes", "read", scope, Duration::from_secs(1_030))
            .map_err(|cause| cause.to_string())?;
        let narrowed = token
            .attenuate(&[extra])
            .map_err(|cause| cause.to_string())?;
        let policy = Policy::new();
        let revocations = RevocationList::new();
        for resource in ["/notes", "/notes/alice", "/notes/alice/inbox", "/notes/bob"] {
            let request =
                Request::new("notes", "read", resource).map_err(|cause| cause.to_string())?;
            let wide = policy
                .decide(&authority, &token, &request, &revocations, &clock)
                .map_err(|cause| cause.to_string())?;
            let narrow = policy
                .decide(&authority, &narrowed, &request, &revocations, &clock)
                .map_err(|cause| cause.to_string())?;
            if narrow.allowed() && !wide.allowed() {
                return Err(format!(
                    "attenuation widened at {resource}: wide denied and narrowed allowed"
                ));
            }
        }
    }
    Ok(())
}

/// Short-lived workload identity issuance carries no static secret in its
/// signature: it is built from the authority alone and lapses on the clock.
#[test]
fn workload_identity_issues_short_lived_token() -> TestResult {
    let (clock, authority, _) = setup(500)?;
    let token = authority
        .issue_workload("edge", "/edge/node-7", Duration::from_secs(20), &clock)
        .map_err(|cause| cause.to_string())?;
    let only = token
        .caveats()
        .first()
        .ok_or_else(|| String::from("a workload token carries its caveat"))?;
    assert_eq!(
        only.action(),
        lgwks_bot::cap_token::WORKLOAD_ACTION,
        "a workload identity names the workload action"
    );
    let revocations = RevocationList::new();
    authority
        .verify(&token, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    let later = clock_at(600);
    match authority.verify(&token, &revocations, &later) {
        Err(TokenError::Expired { .. }) => Ok(()),
        other => Err(format!(
            "a workload identity lapses on the clock: {other:?}"
        )),
    }
}

/// A root-scoped token covers the whole hierarchy, and a sibling prefix
/// never confuses the boundary check.
#[test]
fn root_scope_covers_descendants_and_prefixes_do_not() -> TestResult {
    let clock = clock_at(1_000);
    let authority = Authority::generate().map_err(|cause| cause.to_string())?;
    let token = authority
        .mint("notes", "read", "/", Duration::from_secs(60), &clock)
        .map_err(|cause| cause.to_string())?;
    let policy = Policy::new();
    let revocations = RevocationList::new();
    let deep =
        Request::new("notes", "read", "/notes/alice/inbox").map_err(|cause| cause.to_string())?;
    let allowed = policy
        .decide(&authority, &token, &deep, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    assert!(
        allowed.allowed(),
        "a root-scoped token covers a deep resource: {allowed:?}"
    );
    let narrow = authority
        .mint("notes", "read", "/notes", Duration::from_secs(60), &clock)
        .map_err(|cause| cause.to_string())?;
    let sibling =
        Request::new("notes", "read", "/notes2/inbox").map_err(|cause| cause.to_string())?;
    let refused = policy
        .decide(&authority, &narrow, &sibling, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    assert!(
        !refused.allowed(),
        "a sibling prefix is not a descendant: {refused:?}"
    );
    Ok(())
}

/// A token survives its own bytes: the serialized form decodes to the same
/// token.
#[test]
fn token_survives_its_own_bytes() -> TestResult {
    let (_, _, token) = setup(1_000)?;
    let bytes = token.to_bytes().map_err(|cause| cause.to_string())?;
    assert!(
        bytes.len() <= lgwks_bot::cap_token::MAX_TOKEN_BYTES,
        "a token fits its own bound"
    );
    let restored = BearerToken::from_bytes(bytes.as_slice()).map_err(|cause| cause.to_string())?;
    assert_eq!(token, restored, "a token round-trips through its bytes");
    assert_eq!(token.tag().len(), 32, "the chain tag is thirty-two bytes");
    assert_eq!(token.id().len(), 32, "the token id is thirty-two bytes");
    assert_eq!(
        token.caveats().len(),
        1,
        "a minted token carries its one caveat"
    );
    Ok(())
}

/// Every bound is a typed refusal: too many caveats, over-long and hostile
/// texts, an empty attenuation, an over-long encoding and a full revocation
/// list.
#[test]
fn bounds_are_refused_typed() -> TestResult {
    let (_, _, token) = setup(1_000)?;

    let mut many = Vec::new();
    for depth in 0..=lgwks_bot::cap_token::MAX_CAVEATS {
        let scope = match depth {
            0 => String::from("/notes"),
            _ => format!("/notes/level-{depth}"),
        };
        let caveat = Caveat::new("notes", "read", scope.as_str(), Duration::from_secs(30))
            .map_err(|cause| cause.to_string())?;
        many.push(caveat);
    }
    match token.attenuate(many.as_slice()) {
        Err(TokenError::TooManyCaveats { .. }) => (),
        other => return Err(format!("past the caveat bound must refuse: {other:?}")),
    }
    match token.attenuate(&[]) {
        Err(TokenError::EmptyCaveats) => (),
        other => return Err(format!("an empty attenuation must refuse: {other:?}")),
    }

    let long = "a".repeat(lgwks_bot::cap_token::MAX_TEXT_LEN.saturating_add(1));
    match Caveat::new(long.as_str(), "read", "/notes", Duration::from_secs(30)) {
        Err(TokenError::TextTooLong { .. }) => (),
        other => return Err(format!("an over-long audience must refuse: {other:?}")),
    }
    match Caveat::new("notes", "read me", "/notes", Duration::from_secs(30)) {
        Err(TokenError::BadText { .. }) => (),
        other => return Err(format!("a spaced action must refuse: {other:?}")),
    }
    match Caveat::new("notes", "read", "notes", Duration::from_secs(30)) {
        Err(TokenError::BadText { .. }) => (),
        other => return Err(format!("an unrooted resource must refuse: {other:?}")),
    }
    match Request::new("notes", "read", "/notes//alice") {
        Err(TokenError::BadText { .. }) => (),
        other => return Err(format!("an empty hierarchy segment must refuse: {other:?}")),
    }

    let bytes = token.to_bytes().map_err(|cause| cause.to_string())?;
    let mut oversized = bytes;
    oversized.resize(lgwks_bot::cap_token::MAX_TOKEN_BYTES.saturating_add(1), 0);
    match BearerToken::from_bytes(oversized.as_slice()) {
        Err(TokenError::Malformed) => (),
        other => return Err(format!("an oversized encoding must refuse: {other:?}")),
    }
    match BearerToken::from_bytes(b"not a token") {
        Err(TokenError::Malformed) => (),
        other => return Err(format!("a non-token must refuse: {other:?}")),
    }

    let mut revocations = RevocationList::new();
    assert!(revocations.is_empty(), "a new revocation list is empty");
    let mut stream = Stream(0x5EED_0000_0000_0001);
    for _ in 0..lgwks_bot::cap_token::MAX_REVOKED {
        let mut id = [0_u8; 32];
        for slot in id.iter_mut() {
            let byte = stream.below(256)?;
            *slot = match u8::try_from(byte) {
                Ok(byte) => byte,
                Err(cause) => return Err(cause.to_string()),
            };
        }
        revocations.revoke(&id).map_err(|cause| cause.to_string())?;
    }
    assert_eq!(
        revocations.len(),
        lgwks_bot::cap_token::MAX_REVOKED,
        "the list holds its bound"
    );
    match revocations.revoke(token.id()) {
        Err(TokenError::RevocationFull) => Ok(()),
        other => Err(format!("past the revocation bound must refuse: {other:?}")),
    }
}

/// The port headline in one journey: a token verifies, attenuates narrower,
/// verifies again and decides allow with a trace naming the deciding rule —
/// the bearer, the narrowing and the explanation together.
#[test]
fn bearer_tokens_verify_attenuate_monotonically_and_decide_with_trace() -> TestResult {
    let (clock, authority, token) = setup(1_000)?;
    let revocations = RevocationList::new();
    authority
        .verify(&token, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    let narrowed = token
        .attenuate(&[
            Caveat::new("notes", "read", "/notes/alice", Duration::from_secs(1_030))
                .map_err(|cause| cause.to_string())?,
        ])
        .map_err(|cause| cause.to_string())?;
    authority
        .verify(&narrowed, &revocations, &clock)
        .map_err(|cause| cause.to_string())?;
    let decision = decide_inbox(&authority, &narrowed, &clock)?;
    assert!(
        decision.allowed(),
        "the narrowed token still allows: {decision:?}"
    );
    assert!(
        !decision.trace().is_empty(),
        "every allow carries its trace: {decision:?}"
    );
    assert_eq!(
        decision.trace().first().map(|step| step.rule()),
        Some("signature"),
        "the trace opens at the signature rule: {decision:?}"
    );
    assert!(
        decision
            .trace()
            .iter()
            .any(|step| step.rule() == "resource"),
        "the trace closes at the resource rule: {decision:?}"
    );
    Ok(())
}

/// The same seed replays the same decision and trace hash: three seeded
/// narrowings each decide twice, and each pair agrees exactly.
#[test]
fn same_seed_replays_same_decision_trace_hash() -> TestResult {
    let mut stream = Stream(0xDEC0DE);
    for _ in 0..3 {
        let (clock, authority, token) = setup(1_000)?;
        let pick = stream.below(3)?;
        let scope = match pick {
            0 => "/notes/alice",
            1 => "/notes/alice/inbox",
            _ => "/notes",
        };
        let narrowed = token
            .attenuate(&[
                Caveat::new("notes", "read", scope, Duration::from_secs(1_030))
                    .map_err(|cause| cause.to_string())?,
            ])
            .map_err(|cause| cause.to_string())?;
        let first = decide_inbox(&authority, &narrowed, &clock)?;
        let second = decide_inbox(&authority, &narrowed, &clock)?;
        assert_same_decision(&first, &second)?;
    }
    Ok(())
}
