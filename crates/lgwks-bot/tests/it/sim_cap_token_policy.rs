//! Deterministic simulation suite for capability tokens (#779, INV-BOT-191):
//! the decision walk, the bounds, and the resource boundary under every
//! traversal spelling (ADR 0067).

use lgwks_bot::cap_token::{
    Authority, BearerToken, Caveat, Decision, Policy, Request, RevocationList, TokenError,
};
use lgwks_bot::clock::Clock;
use std::time::Duration;

/// A test failure, in the shape the other modules' tests use.
type TestResult = Result<(), String>;

/// Render any displayable failure as the test's error.
fn show_err<T: std::fmt::Display>(err: T) -> String {
    format!("{err}")
}

/// One virtual clock, one authority and one evaluator per simulated run.
struct SimHarness {
    /// The injected clock every mint and decision reads.
    clock: Clock,
    /// The issuing side.
    authority: Authority,
    /// The evaluator.
    policy: Policy,
}

impl SimHarness {
    /// A run at virtual time zero.
    fn create() -> Result<Self, String> {
        Ok(Self {
            clock: Clock::virtual_at(Duration::ZERO),
            authority: Authority::generate().map_err(show_err)?,
            policy: Policy::new(),
        })
    }

    /// Mint one caveat living `ttl_secs` on the run's clock.
    fn mint(
        &self,
        audience: &str,
        action: &str,
        resource: &str,
        ttl_secs: u64,
    ) -> Result<BearerToken, String> {
        self.authority
            .mint(
                audience,
                action,
                resource,
                Duration::from_secs(ttl_secs),
                &self.clock,
            )
            .map_err(show_err)
    }

    /// Decide one request against `revocations`.
    fn decide_with(
        &self,
        token: &BearerToken,
        revocations: &RevocationList,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<Decision, String> {
        let req = Request::new(audience, action, resource).map_err(show_err)?;
        self.policy
            .decide(&self.authority, token, &req, revocations, &self.clock)
            .map_err(show_err)
    }

    /// Decide one request with nothing revoked.
    fn decide(
        &self,
        token: &BearerToken,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<Decision, String> {
        self.decide_with(token, &RevocationList::new(), audience, action, resource)
    }

    /// Whether one request is allowed with nothing revoked.
    fn is_allowed(
        &self,
        token: &BearerToken,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<bool, String> {
        self.decide(token, audience, action, resource)
            .map(|decision| decision.allowed())
    }
}

/// The rule the walk stopped at.
fn last_rule(decision: &Decision) -> Option<&str> {
    decision.trace().last().map(|step| step.rule())
}

#[test]
fn sim_token_mint_and_verify_deterministic_trace() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:auth", "read", "/res/a", 100)?;
    if !env.is_allowed(&tok, "svc:auth", "read", "/res/a/1")? {
        return Err("expected token to be allowed".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_attenuation_chain_depth_monotonicity() -> TestResult {
    let env = SimHarness::create()?;
    let base = env.mint("svc:data", "read", "/data", 300)?;
    let c1 =
        Caveat::new("svc:data", "read", "/data/sub", Duration::from_secs(200)).map_err(show_err)?;
    let step1 = base.attenuate(&[c1]).map_err(show_err)?;
    let c2 = Caveat::new(
        "svc:data",
        "read",
        "/data/sub/leaf",
        Duration::from_secs(100),
    )
    .map_err(show_err)?;
    let step2 = step1.attenuate(&[c2]).map_err(show_err)?;
    if !env.is_allowed(&step2, "svc:data", "read", "/data/sub/leaf")? {
        return Err("expected attenuated token to allow nested path".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_audience_mismatch_denied() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:worker_a", "read", "/res", 60)?;
    if env.is_allowed(&tok, "svc:worker_b", "read", "/res")? {
        return Err("expected audience mismatch to be denied".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_action_escalation_refused() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:app", "read", "/res", 60)?;
    if env.is_allowed(&tok, "svc:app", "write", "/res")? {
        return Err("expected write action on read token to be denied".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_path_prefix_containment() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:fs", "read", "/store/public", 60)?;
    if !env.is_allowed(&tok, "svc:fs", "read", "/store/public/nested/doc")? {
        return Err("expected subpath to be allowed".to_owned());
    }
    let sibling = env.decide(&tok, "svc:fs", "read", "/store/public-secret/doc")?;
    if sibling.allowed() || last_rule(&sibling) != Some("resource") {
        return Err(format!(
            "a sibling prefix must deny at the resource rule: {sibling:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_path_disjoint_rejection() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("tenant:42", "exec", "/jobs/compute", 50)?;
    if env.is_allowed(&tok, "tenant:42", "exec", "/jobs/admin")? {
        return Err("expected disjoint path to be denied".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_expired_timestamp_evaluated_with_virtual_clock() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("tenant:1", "ping", "/status", 10)?;

    env.clock
        .advance(Duration::from_secs(5))
        .map_err(show_err)?;
    if !env.is_allowed(&tok, "tenant:1", "ping", "/status")? {
        return Err("expected token to be valid at 5s".to_owned());
    }

    env.clock
        .advance(Duration::from_secs(6))
        .map_err(show_err)?;
    let lapsed = env.decide(&tok, "tenant:1", "ping", "/status")?;
    if lapsed.allowed() || last_rule(&lapsed) != Some("expiry") {
        return Err(format!(
            "a token lapsed at 11s must deny at expiry: {lapsed:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_exact_expiry_boundary() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:edge", "call", "/api", 10)?;

    env.clock
        .advance(Duration::from_secs(10))
        .map_err(show_err)?;
    if env.is_allowed(&tok, "svc:edge", "call", "/api")? {
        return Err("expected boundary token at exactly 10s to be expired".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_revocation_list_membership() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:worker", "pull", "/queue", 100)?;
    let mut rev = RevocationList::new();
    rev.revoke(tok.id()).map_err(show_err)?;
    let revoked = env.decide_with(&tok, &rev, "svc:worker", "pull", "/queue")?;
    if revoked.allowed() || last_rule(&revoked) != Some("revocation") {
        return Err(format!(
            "a revoked token must deny at revocation: {revoked:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_revocation_idempotent_duplicate_entry() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:worker", "pull", "/queue", 100)?;
    let mut rev = RevocationList::new();
    rev.revoke(tok.id()).map_err(show_err)?;
    rev.revoke(tok.id()).map_err(show_err)?;
    assert_eq!(rev.len(), 1, "a repeated revocation lists the id once");
    if env
        .decide_with(&tok, &rev, "svc:worker", "pull", "/queue")?
        .allowed()
    {
        return Err("expected revoked token to stay denied after duplicate revoke".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_tampered_signature_byte_invalidation() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:db", "select", "/table", 100)?;
    let mut serialized = tok.to_bytes().map_err(show_err)?;
    let at = serialized
        .len()
        .checked_sub(3)
        .ok_or("a token serializes past three bytes")?;
    let digit = serialized.get_mut(at).ok_or("the tag's last digit")?;
    *digit = if *digit == b'0' { b'1' } else { b'0' };
    match BearerToken::from_bytes(&serialized) {
        Ok(forged) => match env
            .authority
            .verify(&forged, &RevocationList::new(), &env.clock)
        {
            Err(TokenError::Tampered) => Ok(()),
            other => Err(format!(
                "a flipped tag digit must verify tampered: {other:?}"
            )),
        },
        Err(other) => Err(format!("a digit swap keeps valid JSON: {other:?}")),
    }
}

#[test]
fn sim_token_truncated_payload_refusal() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:db", "select", "/table", 100)?;
    let serialized = tok.to_bytes().map_err(show_err)?;
    let half_len = serialized.len() >> 1;
    let truncated = &serialized[..half_len];
    match BearerToken::from_bytes(truncated) {
        Err(TokenError::Malformed) => Ok(()),
        other => Err(format!(
            "a truncated payload must refuse malformed: {other:?}"
        )),
    }
}

#[test]
fn sim_token_multi_caveat_evaluation_order() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:all", "read", "/root", 100)?;
    let c1 = Caveat::new("svc:all", "read", "/root/branch", Duration::from_secs(80))
        .map_err(show_err)?;
    let c2 = Caveat::new(
        "svc:all",
        "read",
        "/root/branch/leaf",
        Duration::from_secs(50),
    )
    .map_err(show_err)?;
    let att = tok.attenuate(&[c1, c2]).map_err(show_err)?;

    if !env.is_allowed(&att, "svc:all", "read", "/root/branch/leaf")? {
        return Err("expected valid subpath to be allowed".to_owned());
    }
    let other = env.decide(&att, "svc:all", "read", "/root/branch/other")?;
    let names_leaf = other
        .trace()
        .last()
        .is_some_and(|step| step.inputs().starts_with("grant=/root/branch/leaf "));
    if other.allowed() || !names_leaf {
        return Err(format!(
            "the denial must name the narrowest grant it missed: {other:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_root_resource_matching() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:gateway", "forward", "/", 100)?;
    if !env.is_allowed(&tok, "svc:gateway", "forward", "/any/path")? {
        return Err("expected root prefix to match request".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_empty_audience_validation() -> TestResult {
    let env = SimHarness::create()?;
    match env
        .authority
        .mint("", "read", "/path", Duration::from_secs(10), &env.clock)
    {
        Err(TokenError::TextTooLong {
            field: "audience", ..
        }) => Ok(()),
        other => Err(format!("an empty audience must refuse typed: {other:?}")),
    }
}

#[test]
fn sim_token_serialization_roundtrip_stability() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:test", "action", "/path/item", 60)?;
    let bytes = tok.to_bytes().map_err(show_err)?;
    let deserialized = BearerToken::from_bytes(&bytes).map_err(show_err)?;
    if tok.id() != deserialized.id() {
        return Err("expected roundtripped token id to match".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_cloned_authority_independent_root() -> TestResult {
    let clock = Clock::virtual_at(Duration::ZERO);
    let a1 = Authority::generate().map_err(show_err)?;
    let a2 = Authority::generate().map_err(show_err)?;
    let t1 = a1
        .mint("svc:auth", "read", "/res", Duration::from_secs(60), &clock)
        .map_err(show_err)?;
    a1.verify(&t1, &RevocationList::new(), &clock)
        .map_err(|cause| format!("authority 1 verifies its own token: {cause}"))?;
    match a2.verify(&t1, &RevocationList::new(), &clock) {
        Err(TokenError::Tampered) => Ok(()),
        other => Err(format!(
            "authority 2 must refuse a foreign token: {other:?}"
        )),
    }
}

#[test]
fn sim_token_cross_authority_signature_rejection() -> TestResult {
    let env = SimHarness::create()?;
    let foreign_auth = Authority::generate().map_err(show_err)?;
    let tok = env.mint("svc:shared", "read", "/common", 60)?;
    let req = Request::new("svc:shared", "read", "/common").map_err(show_err)?;
    let decision = env
        .policy
        .decide(
            &foreign_auth,
            &tok,
            &req,
            &RevocationList::new(),
            &env.clock,
        )
        .map_err(show_err)?;
    if decision.allowed() || last_rule(&decision) != Some("signature") {
        return Err(format!(
            "a foreign authority must deny at the signature: {decision:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_hierarchical_subpath_delegation() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("tenant:corp", "manage", "/billing/v2", 120)?;
    if !env.is_allowed(&tok, "tenant:corp", "manage", "/billing/v2/invoices/99")? {
        return Err("expected nested subpath request to be allowed".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_evaluator_trace_records_reasons() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:tracer", "read", "/logs", 60)?;
    let decision = env.decide(&tok, "svc:tracer", "write", "/logs")?;
    if decision.allowed() || last_rule(&decision) != Some("action") {
        return Err(format!(
            "an action mismatch must deny at the action rule: {decision:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_clock_monotonic_advance_transitions() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("client:1", "query", "/search", 50)?;

    if !env.is_allowed(&tok, "client:1", "query", "/search")? {
        return Err("expected allow at T=0".to_owned());
    }

    env.clock
        .advance(Duration::from_secs(25))
        .map_err(show_err)?;
    if !env.is_allowed(&tok, "client:1", "query", "/search")? {
        return Err("expected allow at T=25".to_owned());
    }

    env.clock
        .advance(Duration::from_secs(26))
        .map_err(show_err)?;
    if env.is_allowed(&tok, "client:1", "query", "/search")? {
        return Err("expected denial at T=51 past 50s TTL".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_zero_duration_instant_expiry() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("client:instant", "ping", "/zero", 0)?;
    let decision = env.decide(&tok, "client:instant", "ping", "/zero")?;
    if decision.allowed() || last_rule(&decision) != Some("expiry") {
        return Err(format!(
            "a zero-duration token must deny at expiry: {decision:?}"
        ));
    }
    Ok(())
}

#[test]
fn sim_token_unrelated_tenant_isolation() -> TestResult {
    let env = SimHarness::create()?;
    let tok_a = env.mint("tenant:alpha", "read", "/tenant/alpha", 60)?;
    let tok_b = env.mint("tenant:beta", "read", "/tenant/alpha", 60)?;
    let foreign = env.decide(&tok_a, "tenant:beta", "read", "/tenant/alpha")?;
    if foreign.allowed() || last_rule(&foreign) != Some("audience") {
        return Err(format!(
            "beta presenting alpha's token must deny at audience: {foreign:?}"
        ));
    }
    if !env.is_allowed(&tok_b, "tenant:beta", "read", "/tenant/alpha")? {
        return Err("beta's own token for the same resource id still allows".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_consecutive_attenuation_narrowing() -> TestResult {
    let env = SimHarness::create()?;
    let t0 = env.mint("bot:pool", "invoke", "/ai/models", 100)?;
    let c1 = Caveat::new(
        "bot:pool",
        "invoke",
        "/ai/models/fast",
        Duration::from_secs(80),
    )
    .map_err(show_err)?;
    let t1 = t0.attenuate(&[c1]).map_err(show_err)?;
    let c2 = Caveat::new(
        "bot:pool",
        "invoke",
        "/ai/models/fast/v1",
        Duration::from_secs(60),
    )
    .map_err(show_err)?;
    let t2 = t1.attenuate(&[c2]).map_err(show_err)?;

    if !env.is_allowed(&t2, "bot:pool", "invoke", "/ai/models/fast/v1")? {
        return Err("expected multi-step attenuated token to allow target".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_decision_disposition_allow_deny_trace() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("agent:dispatch", "exec", "/tasks", 60)?;
    let allowed = env.decide(&tok, "agent:dispatch", "exec", "/tasks/run")?;
    if !allowed.allowed() || last_rule(&allowed) != Some("resource") {
        return Err(format!("an allow walks to the resource rule: {allowed:?}"));
    }
    let denied = env.decide(&tok, "agent:other", "exec", "/tasks/run")?;
    if denied.allowed() || last_rule(&denied) != Some("audience") {
        return Err(format!("a foreign audience denies at audience: {denied:?}"));
    }
    Ok(())
}

/// CWE-23 relative path traversal: a segment that climbs out of the scope is
/// refused at the boundary, so `/notes/alice/../bob` never reaches the policy
/// a token scoped to `/notes/alice` would judge it under.
#[test]
fn a_dot_dot_segment_is_refused_typed() -> TestResult {
    match Request::new("notes", "read", "/notes/alice/../bob") {
        Err(TokenError::BadText { field: "resource" }) => (),
        other => return Err(format!("a climbing request must refuse: {other:?}")),
    }
    match Caveat::new("notes", "read", "/notes/..", Duration::from_secs(30)) {
        Err(TokenError::BadText { field: "resource" }) => Ok(()),
        other => Err(format!("a climbing caveat must refuse: {other:?}")),
    }
}

/// CWE-28..35 spell traversal with backslashes and single, triple and
/// multiple dots, and CWE-174 with percent encoding: every spelling is refused
/// the same way on every platform, and a dot inside a name is still admitted.
#[test]
fn every_traversal_spelling_is_refused() -> TestResult {
    for hostile in [
        "/notes/alice\\..\\bob",
        "/notes/alice/.",
        "/notes/...",
        "/notes/..../bob",
        "/notes/%2e%2e/bob",
        "/notes/alice%2fbob",
    ] {
        match Request::new("notes", "read", hostile) {
            Err(TokenError::BadText { field: "resource" }) => (),
            other => return Err(format!("{hostile} must refuse BadText: {other:?}")),
        }
    }
    for admitted in ["/notes/alice.txt", "/notes/.profile", "/notes/v1..2"] {
        Request::new("notes", "read", admitted)
            .map_err(|cause| format!("{admitted} names a node: {cause}"))?;
    }
    Ok(())
}

/// A request decoded from the wire is validated like one built by
/// `Request::new`: deserialization is not a way around the boundary.
#[test]
fn a_deserialized_request_is_validated() -> TestResult {
    let hostile = br#"{"audience":"notes","action":"read","resource":"/notes/../admin"}"#;
    if let Ok(request) = lgwks_std::json::from_slice::<Request>(hostile) {
        return Err(format!(
            "a climbing request decoded unvalidated: {request:?}"
        ));
    }
    let honest = br#"{"audience":"notes","action":"read","resource":"/notes/admin"}"#;
    let request: Request = lgwks_std::json::from_slice(honest).map_err(show_err)?;
    assert_eq!(
        request.resource(),
        "/notes/admin",
        "a valid request decodes whole"
    );
    Ok(())
}

/// CWE-532: an authority written to a log shows its type and never its key.
#[test]
fn authority_debug_redacts_the_root_key() -> TestResult {
    let authority = Authority::generate().map_err(show_err)?;
    assert_eq!(
        format!("{authority:?}"),
        "Authority { root: <redacted> }",
        "the root key never reaches a Debug line"
    );
    let env = SimHarness::create()?;
    let token = env.mint("svc:log", "read", "/logs", 60)?;
    let line = format!("{token:?}");
    let tag = lgwks_std::hex::encode(token.tag());
    assert!(
        line.contains("tag: <redacted>") && !line.contains(tag.as_str()),
        "a bearer token in a Debug line carries no replayable tag: {line}"
    );
    Ok(())
}

/// A token at the caveat bound decides in the same six steps as a fresh one,
/// each step's inputs inside the text bound: a decision's work and trace do not
/// grow with the delegation chain.
#[test]
fn a_full_chain_decides_within_its_trace_bound() -> TestResult {
    let env = SimHarness::create()?;
    let mut token = env.mint("svc:chain", "read", "/c", 600)?;
    let mut scope = String::from("/c");
    while token.caveats().len() < lgwks_bot::cap_token::MAX_CAVEATS {
        scope.push_str("/n");
        let caveat = Caveat::new(
            "svc:chain",
            "read",
            scope.as_str(),
            Duration::from_secs(600),
        )
        .map_err(show_err)?;
        token = token.attenuate(&[caveat]).map_err(show_err)?;
    }
    let decision = env.decide(&token, "svc:chain", "read", scope.as_str())?;
    assert!(
        decision.allowed(),
        "the deepest scope is covered: {decision:?}"
    );
    assert_eq!(
        decision.trace().len(),
        6,
        "the walk is six steps at any depth"
    );
    let widest = decision
        .trace()
        .iter()
        .map(|step| step.inputs().len())
        .max();
    assert!(
        widest <= Some(lgwks_bot::cap_token::MAX_TEXT_LEN),
        "every step's inputs fit the text bound: {decision:?}"
    );
    match token.attenuate(&[
        Caveat::new("svc:chain", "read", "/c", Duration::from_secs(1)).map_err(show_err)?,
    ]) {
        Err(TokenError::TooManyCaveats { .. }) => Ok(()),
        other => Err(format!("one past the caveat bound must refuse: {other:?}")),
    }
}

/// SplitMix64: one seeded word per call, no wall clock, no OS entropy.
fn mix(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut word = *state;
    word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    word ^ (word >> 31)
}

/// A draw below `bound` from the seeded stream.
fn draw(state: &mut u64, bound: u64) -> Result<usize, String> {
    let value = mix(state)
        .checked_rem(bound)
        .ok_or("a zero bound draws nothing")?;
    usize::try_from(value).map_err(show_err)
}

/// The model the boundary must match: a segment is hostile when it is only
/// dots, or holds a backslash or a percent.
fn hostile_segment(segment: &str) -> bool {
    segment.bytes().all(|byte| byte == b'.') || segment.contains(['\\', '%'])
}

/// Log why a sweep stopped and hand the reason back as the test's error.
fn refuse<T>(why: String) -> Result<T, String> {
    lgwks_std::trace::warn!(%why, "sim_cap_token_policy: the sweep left the segment model");
    Err(why)
}

/// Sweep `count` seeded resources through `Request::new`, check each verdict
/// against the model, and fold the verdicts into one hash.
fn sweep(seed: u64, count: usize) -> Result<[u8; 32], String> {
    const ALPHABET: [char; 6] = ['a', 'b', '.', '\\', '%', '-'];
    let mut state = seed;
    let mut hasher = lgwks_std::hash::Hasher::new();
    for _ in 0..count {
        let mut resource = String::new();
        let mut hostile = false;
        for _ in 0..=draw(&mut state, 4)? {
            let mut segment = String::new();
            for _ in 0..=draw(&mut state, 4)? {
                segment.push(
                    *ALPHABET
                        .get(draw(&mut state, 6)?)
                        .ok_or("an alphabet slot")?,
                );
            }
            hostile = hostile || hostile_segment(segment.as_str());
            resource.push('/');
            resource.push_str(segment.as_str());
        }
        let admitted = match Request::new("svc:sweep", "read", resource.as_str()) {
            Ok(_) => true,
            Err(TokenError::BadText { field: "resource" }) => false,
            Err(other) => return refuse(format!("{resource} refused untyped: {other}")),
        };
        if admitted == hostile {
            return refuse(format!(
                "{resource}: admitted={admitted} but the model says hostile={hostile}"
            ));
        }
        hasher.write_framed(resource.as_bytes());
        hasher.write_framed(&[u8::from(admitted)]);
    }
    Ok(*hasher.finalize().as_bytes())
}

/// 4,096 seeded resources match the segment model, the same seed replays the
/// same verdict hash, and another seed gives another one.
#[test]
fn sim_token_seeded_resource_sweep_matches_the_segment_model() -> TestResult {
    let first = sweep(0x5EED_CAFE, 4_096)?;
    let again = sweep(0x5EED_CAFE, 4_096)?;
    let other = sweep(0x5EED_CAFF, 4_096)?;
    assert_eq!(first, again, "the same seed replays the same verdicts");
    assert_ne!(
        first, other,
        "the verdict hash reads the generated resources"
    );
    Ok(())
}
