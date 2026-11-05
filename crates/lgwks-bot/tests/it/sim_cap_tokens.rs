//! Deterministic simulation suite for capability tokens (#779, INV-BOT-191).

use lgwks_bot::cap_token::{
    Authority,
    BearerToken,
    Caveat,
    Decision,
    Policy,
    Request,
    // live revocation
    RevocationList,
};
use lgwks_bot::clock::Clock;
use std::time::Duration;

type TestResult = Result<(), String>;

fn show_err<T: std::fmt::Display>(err: T) -> String {
    format!("{err}")
}

struct SimHarness {
    clock: Clock,
    authority: Authority,
    policy: Policy,
}

impl SimHarness {
    fn create() -> Result<Self, String> {
        Ok(Self {
            clock: Clock::virtual_at(Duration::ZERO),
            authority: Authority::generate().map_err(show_err)?,
            policy: Policy::new(),
        })
    }

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

    // live revocation helper for simulated decisions
    fn decide_request(
        &self,
        token: &BearerToken,
        rev: Option<&RevocationList>,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<Decision, String> {
        let req = Request::new(audience, action, resource).map_err(show_err)?;
        // live revocation
        let fallback = RevocationList::new();
        let target_rev = match rev {
            Some(rev_ref) => rev_ref,
            None => &fallback,
        };
        self.policy
            .decide(&self.authority, token, &req, target_rev, &self.clock)
            .map_err(show_err)
    }

    fn is_allowed(
        &self,
        token: &BearerToken,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<bool, String> {
        self.decide_request(token, None, audience, action, resource)
            .map(|decision| decision.allowed())
    }

    // live revocation evaluation helper
    fn is_allowed_with_rev(
        &self,
        token: &BearerToken,
        rev: &RevocationList,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<bool, String> {
        self.decide_request(token, Some(rev), audience, action, resource)
            .map(|decision| decision.allowed())
    }

    fn decide_trace(
        &self,
        token: &BearerToken,
        audience: &str,
        action: &str,
        resource: &str,
    ) -> Result<(bool, bool), String> {
        self.decide_request(token, None, audience, action, resource)
            .map(|decision| (decision.allowed(), !decision.trace().is_empty()))
    }
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
    if env.is_allowed(&tok, "svc:fs", "read", "/store/public-secret/doc")? {
        return Err("expected sibling non-subpath to be denied".to_owned());
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
    if env.is_allowed(&tok, "tenant:1", "ping", "/status")? {
        return Err("expected expired token at 11s to be denied".to_owned());
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
// live revocation
fn sim_token_revocation_list_membership() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:worker", "pull", "/queue", 100)?;
    // live revocation
    let mut rev = RevocationList::new();
    // live revocation
    rev.revoke(tok.id()).map_err(show_err)?;

    if env.is_allowed_with_rev(&tok, &rev, "svc:worker", "pull", "/queue")? {
        // live revocation
        return Err("expected revoked token to be denied".to_owned());
    }
    Ok(())
}

#[test]
// live revocation
fn sim_token_revocation_idempotent_duplicate_entry() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:worker", "pull", "/queue", 100)?;
    // live revocation
    let mut rev = RevocationList::new();
    // live revocation
    rev.revoke(tok.id()).map_err(show_err)?;
    // live revocation
    rev.revoke(tok.id()).map_err(show_err)?;

    if env.is_allowed_with_rev(&tok, &rev, "svc:worker", "pull", "/queue")? {
        // live revocation
        return Err("expected revoked token to stay denied after duplicate revoke".to_owned());
    }
    Ok(())
}

#[test]
fn sim_token_tampered_signature_byte_invalidation() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:db", "select", "/table", 100)?;
    let mut serialized = tok.to_bytes().map_err(show_err)?;
    if let Some(byte) = serialized.last_mut() {
        *byte ^= 0x5a;
    }
    let tampered = BearerToken::from_bytes(&serialized);
    match tampered {
        Ok(tampered_token) if env.is_allowed(&tampered_token, "svc:db", "select", "/table")? => {
            return Err("expected tampered signature token to be rejected".to_owned());
        }
        _ => {}
    }
    Ok(())
}

#[test]
fn sim_token_truncated_payload_refusal() -> TestResult {
    let env = SimHarness::create()?;
    let tok = env.mint("svc:db", "select", "/table", 100)?;
    let serialized = tok.to_bytes().map_err(show_err)?;
    let half_len = serialized.len() >> 1;
    let truncated = &serialized[..half_len];
    if BearerToken::from_bytes(truncated).is_ok() {
        return Err("expected truncated payload to fail parsing".to_owned());
    }
    Ok(())
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
    if env.is_allowed(&att, "svc:all", "read", "/root/branch/other")? {
        return Err("expected non-matching leaf path to be denied".to_owned());
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
    let res = env
        .authority
        .mint("", "read", "/path", Duration::from_secs(10), &env.clock);
    if res.is_ok() {
        return Err("expected empty audience to be refused".to_owned());
    }
    Ok(())
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
// live revocation
fn sim_token_cloned_authority_independent_root() -> TestResult {
    let clock = Clock::virtual_at(Duration::ZERO);
    let a1 = Authority::generate().map_err(show_err)?;
    let a2 = Authority::generate().map_err(show_err)?;
    let t1 = a1
        .mint("svc:auth", "read", "/res", Duration::from_secs(60), &clock)
        .map_err(show_err)?;
    // live revocation
    if a1.verify(&t1, &RevocationList::new(), &clock).is_err() {
        return Err("expected authority 1 to verify its own token".to_owned());
    }
    // live revocation
    if a2.verify(&t1, &RevocationList::new(), &clock).is_ok() {
        return Err("expected authority 2 to refuse token minted by authority 1".to_owned());
    }
    Ok(())
}

#[test]
// live revocation
fn sim_token_cross_authority_signature_rejection() -> TestResult {
    let env = SimHarness::create()?;
    let foreign_auth = Authority::generate().map_err(show_err)?;
    let tok = env.mint("svc:shared", "read", "/common", 60)?;
    let req = Request::new("svc:shared", "read", "/common").map_err(show_err)?;
    // live revocation
    let dec = env.policy.decide(
        &foreign_auth,
        &tok,
        &req,
        // live revocation
        &RevocationList::new(),
        &env.clock,
    );
    match dec {
        Ok(decision) if decision.allowed() => {
            return Err("expected foreign authority decision to deny".to_owned());
        }
        _ => {}
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
    let (allowed, has_trace) = env.decide_trace(&tok, "svc:tracer", "write", "/logs")?;
    if allowed {
        return Err("expected denial for action mismatch".to_owned());
    }
    if !has_trace {
        return Err("expected explanation trace to be non-empty".to_owned());
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
    let res = env.authority.mint(
        "client:instant",
        "ping",
        "/zero",
        Duration::ZERO,
        &env.clock,
    );
    match res {
        Ok(tok) if env.is_allowed(&tok, "client:instant", "ping", "/zero")? => {
            return Err("expected zero-duration token to be expired".to_owned());
        }
        _ => {}
    }
    Ok(())
}

#[test]
fn sim_token_unrelated_tenant_isolation() -> TestResult {
    let env = SimHarness::create()?;
    let tok_a = env.mint("tenant:alpha", "read", "/tenant/alpha", 60)?;
    if env.is_allowed(&tok_a, "tenant:beta", "read", "/tenant/alpha")? {
        return Err("expected cross-tenant request to be denied".to_owned());
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
    let (allowed_ok, trace_ok) = env.decide_trace(&tok, "agent:dispatch", "exec", "/tasks/run")?;
    if !allowed_ok || !trace_ok {
        return Err("expected allow with trace".to_owned());
    }

    let (allowed_bad, trace_bad) = env.decide_trace(&tok, "agent:other", "exec", "/tasks/run")?;
    if allowed_bad || !trace_bad {
        return Err("expected deny with trace".to_owned());
    }
    Ok(())
}
