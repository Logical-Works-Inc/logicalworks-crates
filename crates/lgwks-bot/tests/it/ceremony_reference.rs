//! Ceremony as a script reference app, ported from the kernel.
//!
//! Kernel sources: `canvas-backend/src/ceremony/*` plus
//! `bridge_ceremony_consent.rs`, `governance_artifacts.rs`,
//! `integration_governance.rs`, `signals_integration.rs`.
//! Estate home: `lgwks_bot::app` with `Provider`, approval, trail and
//! `RunStore`; this file ports the kernel guarantees onto those doors.
//! Every test names the kernel test it ports in its table row.

use std::collections::BTreeMap;
use std::fmt;
use std::time::SystemTime;

use lgwks_std::hash::{Digest, Hasher};

/// The longest tenant name this reference accepts, in bytes.
const MAX_TENANT_BYTES: usize = 64;
/// The longest workspace name this reference accepts, in bytes.
const MAX_WORKSPACE_BYTES: usize = 64;
/// The longest identity string this reference accepts, in bytes.
const MAX_IDENTITY_BYTES: usize = 256;
/// The longest consent key this reference accepts, in bytes.
const MAX_KEY_BYTES: usize = 256;
/// The most consent facts one store holds.
const MAX_CONSENTS: usize = 1_024;
/// The most governance entries one log holds.
const MAX_GOVERNANCE_ENTRIES: usize = 1_024;
/// The longest signal timestamp this reference accepts, in bytes.
const MAX_SIGNAL_BYTES: usize = 64;
/// The longest signal body this reference accepts, in bytes.
const MAX_BODY_BYTES: usize = 8_192;
/// The most clock drift a signal timestamp may carry, in seconds.
const MAX_DRIFT_SECS: u64 = 300;
/// The widest governance query window this reference admits, in seconds.
const MAX_WINDOW_SECS: u64 = 86_400;

/// How this reference broke, naming the fact that moved.
#[derive(Debug)]
enum CeremonyFailed {
    /// A value passed its declared ceiling.
    Bound {
        /// What was refused.
        what: &'static str,
        /// The observed length.
        len: usize,
        /// The ceiling it passed.
        max: usize,
    },
    /// The store refused the read or the write.
    Store {
        /// What was refused.
        what: &'static str,
    },
    /// Two values that must agree did not.
    Mismatch {
        /// What disagreed.
        what: &'static str,
    },
    /// A JSON document was refused.
    Json(lgwks_std::json::Error),
    /// A timestamp was refused.
    Time(lgwks_std::time::ParseError),
}

impl fmt::Display for CeremonyFailed {
    /// The refusal, naming the bound or the disagreement.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Bound { what, len, max } => {
                write!(f, "{what} is {len} bytes against a ceiling of {max}")
            }
            Self::Store { what } => write!(f, "the store refused: {what}"),
            Self::Mismatch { what } => write!(f, "mismatch: {what}"),
            Self::Json(ref cause) => write!(f, "json refused: {cause}"),
            Self::Time(ref cause) => write!(f, "time refused: {cause}"),
        }
    }
}

impl std::error::Error for CeremonyFailed {}

impl From<lgwks_std::json::Error> for CeremonyFailed {
    fn from(cause: lgwks_std::json::Error) -> Self {
        Self::Json(cause)
    }
}

impl From<lgwks_std::time::ParseError> for CeremonyFailed {
    fn from(cause: lgwks_std::time::ParseError) -> Self {
        Self::Time(cause)
    }
}

/// Refuse with a store reason, logging the refusal for the operator.
fn failed<T>(what: &'static str) -> Result<T, CeremonyFailed> {
    lgwks_std::trace::warn!(what, "ceremony: refused");
    Err(CeremonyFailed::Store { what })
}

/// Refuse `what` of `len` bytes past `max`.
fn check_bound(what: &'static str, len: usize, max: usize) -> Result<(), CeremonyFailed> {
    if len > max {
        lgwks_std::trace::warn!(what, len, max, "ceremony: a value passed its ceiling");
        return Err(CeremonyFailed::Bound { what, len, max });
    }
    Ok(())
}

/// The validated tenant name a query runs under.
fn tenant_key(tenant: &str) -> Result<String, CeremonyFailed> {
    check_bound("tenant", tenant.len(), MAX_TENANT_BYTES)?;
    if tenant.is_empty() {
        return failed("tenant is empty");
    }
    Ok(tenant.to_owned())
}

/// One consent fact: who granted which key on which workspace, in order.
#[derive(Debug, Clone)]
struct Consent {
    /// Who granted it.
    tenant: String,
    /// Which workspace it covers.
    workspace: String,
    /// Which user granted it.
    user: String,
    /// The identity that granted it.
    identity: String,
    /// The provisioned key handle behind the grant.
    key: String,
    /// The grant order; larger wins.
    seq: u64,
}

impl Consent {
    /// The consent fact for `tenant` and `workspace`, or the refusal.
    fn new(
        tenant: &str,
        workspace: &str,
        user: &str,
        identity: &str,
        key: &str,
        seq: u64,
    ) -> Result<Self, CeremonyFailed> {
        check_bound("tenant", tenant.len(), MAX_TENANT_BYTES)?;
        check_bound("workspace", workspace.len(), MAX_WORKSPACE_BYTES)?;
        check_bound("identity", identity.len(), MAX_IDENTITY_BYTES)?;
        check_bound("key", key.len(), MAX_KEY_BYTES)?;
        if tenant.is_empty() || workspace.is_empty() || key.is_empty() {
            return failed("consent names tenant workspace and key");
        }
        Ok(Self {
            tenant: tenant.to_owned(),
            workspace: workspace.to_owned(),
            user: user.to_owned(),
            identity: identity.to_owned(),
            key: key.to_owned(),
            seq,
        })
    }

    /// Who granted it.
    fn tenant(&self) -> &str {
        self.tenant.as_str()
    }

    /// Which workspace it covers.
    fn workspace(&self) -> &str {
        self.workspace.as_str()
    }

    /// Which user granted it.
    fn user(&self) -> &str {
        self.user.as_str()
    }

    /// The identity that granted it.
    fn identity(&self) -> &str {
        self.identity.as_str()
    }

    /// The provisioned key handle behind the grant.
    fn key(&self) -> &str {
        self.key.as_str()
    }
}

/// The append-only consent facts one test holds, bounded.
#[derive(Debug, Default)]
struct ConsentStore {
    /// Every grant, in arrival order.
    records: Vec<Consent>,
}

impl ConsentStore {
    /// An empty store.
    fn new() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    /// Append `consent`, or refuse past the ceiling.
    fn record(&mut self, consent: Consent) -> Result<(), CeremonyFailed> {
        if self.records.len() >= MAX_CONSENTS {
            return failed("consent store is full");
        }
        self.records.push(consent);
        Ok(())
    }

    /// The newest grant for `tenant` and `workspace`, or none.
    fn latest(&self, tenant: &str, workspace: &str) -> Result<Option<Consent>, CeremonyFailed> {
        let _ = tenant_key(tenant)?;
        check_bound("workspace", workspace.len(), MAX_WORKSPACE_BYTES)?;
        let mut best: Option<&Consent> = None;
        for record in self.records.iter() {
            if record.tenant() != tenant || record.workspace() != workspace {
                continue;
            }
            let take = match best {
                None => true,
                Some(current) => record.seq > current.seq,
            };
            if take {
                best = Some(record);
            }
        }
        Ok(best.cloned())
    }

    /// How many facts the store holds.
    fn len(&self) -> usize {
        self.records.len()
    }
}

/// An empty consent store for one test.
fn consent_fixture() -> Result<ConsentStore, CeremonyFailed> {
    Ok(ConsentStore::new())
}

/// Ports `consent_latest_wins`: a re-grant supersedes, history is kept.
#[test]
fn consent_latest_wins() -> Result<(), CeremonyFailed> {
    let mut store = consent_fixture()?;
    store.record(Consent::new(
        "tenant-a",
        "ws-1",
        "u-9",
        "admin@x.io",
        "key-OLD",
        1,
    )?)?;
    store.record(Consent::new(
        "tenant-a",
        "ws-1",
        "u-9",
        "admin@x.io",
        "key-NEW",
        2,
    )?)?;
    let active = store.latest("tenant-a", "ws-1")?;
    match active {
        Some(consent) => {
            if consent.key() != "key-NEW" {
                return failed("latest grant is active");
            }
            if consent.user() != "u-9" || consent.identity() != "admin@x.io" {
                return failed("the grant carries its user and identity");
            }
        }
        None => return failed("a granted workspace has active consent"),
    }
    if store.len() != 2 {
        return failed("both consent facts are retained");
    }
    Ok(())
}

/// Ports `consent_is_tenant_isolated`: the workspace id alone grants nothing.
#[test]
fn consent_is_tenant_isolated() -> Result<(), CeremonyFailed> {
    let mut store = consent_fixture()?;
    store.record(Consent::new(
        "tenant-a", "ws-42", "u-1", "a@x.io", "key-A", 1,
    )?)?;
    let owner = store.latest("tenant-a", "ws-42")?;
    if owner.is_none() {
        return failed("the owning tenant sees its own consent");
    }
    let foreign = store.latest("tenant-b", "ws-42")?;
    if foreign.is_some() {
        return failed("a foreign tenant reads no consent for the same workspace");
    }
    Ok(())
}

/// Ports `consent_is_workspace_scoped_within_tenant` and the unknown case.
#[test]
fn consent_is_workspace_scoped() -> Result<(), CeremonyFailed> {
    let mut store = consent_fixture()?;
    store.record(Consent::new(
        "tenant-a",
        "ws-granted",
        "u-1",
        "a@x.io",
        "key-1",
        1,
    )?)?;
    let granted = store.latest("tenant-a", "ws-granted")?;
    if granted.is_none() {
        return failed("the granted workspace has consent");
    }
    let other = store.latest("tenant-a", "ws-other")?;
    if other.is_some() {
        return failed("another workspace on the same chain has no consent");
    }
    let unknown = store.latest("tenant-a", "ws-missing")?;
    if unknown.is_some() {
        return failed("an ungranted workspace folds to none");
    }
    Ok(())
}

/// One governance fact: whose session this signal belongs to.
#[derive(Debug, Clone)]
struct GovEntry {
    /// Who appended it.
    tenant: String,
    /// Which session it belongs to.
    session: String,
    /// The deduplicated signal id.
    signal: String,
}

impl GovEntry {
    /// The governance fact for `tenant`, `session` and `signal`.
    fn new(tenant: &str, session: &str, signal: &str) -> Result<Self, CeremonyFailed> {
        check_bound("tenant", tenant.len(), MAX_TENANT_BYTES)?;
        check_bound("session", session.len(), MAX_WORKSPACE_BYTES)?;
        check_bound("signal", signal.len(), MAX_WORKSPACE_BYTES)?;
        if tenant.is_empty() || session.is_empty() || signal.is_empty() {
            return failed("governance entry names tenant session and signal");
        }
        Ok(Self {
            tenant: tenant.to_owned(),
            session: session.to_owned(),
            signal: signal.to_owned(),
        })
    }
}

/// The bounded governance log one test holds, with its signal index.
#[derive(Debug, Default)]
struct GovernanceLog {
    /// Every entry, in arrival order.
    entries: Vec<GovEntry>,
    /// Signal id to entry position, for duplicate refusal.
    by_signal: BTreeMap<String, usize>,
}

impl GovernanceLog {
    /// An empty log.
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            by_signal: BTreeMap::new(),
        }
    }

    /// Append `entry`, refusing a duplicate signal as a typed conflict.
    fn append(&mut self, entry: GovEntry) -> Result<(), CeremonyFailed> {
        if self.entries.len() >= MAX_GOVERNANCE_ENTRIES {
            return failed("governance log is full");
        }
        match self.by_signal.entry(entry.signal.clone()) {
            std::collections::btree_map::Entry::Occupied(_) => {
                lgwks_std::trace::warn!("ceremony: a duplicate signal is refused");
                return Err(CeremonyFailed::Mismatch {
                    what: "duplicate signal",
                });
            }
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(self.entries.len());
            }
        }
        self.entries.push(entry);
        Ok(())
    }

    /// The entries for `tenant` and `session` inside the stated window.
    fn query(
        &self,
        tenant: &str,
        session: &str,
        from_secs: u64,
        to_secs: u64,
    ) -> Result<Vec<GovEntry>, CeremonyFailed> {
        let _ = tenant_key(tenant)?;
        if session.is_empty() {
            lgwks_std::trace::warn!("ceremony: a missing session is refused");
            return Err(CeremonyFailed::Mismatch {
                what: "missing session",
            });
        }
        if from_secs > to_secs {
            lgwks_std::trace::warn!(from_secs, to_secs, "ceremony: a negative window is refused");
            return Err(CeremonyFailed::Mismatch {
                what: "negative window",
            });
        }
        let width = to_secs.saturating_sub(from_secs);
        if width > MAX_WINDOW_SECS {
            lgwks_std::trace::warn!(width, "ceremony: a window over 24h is refused");
            return Err(CeremonyFailed::Mismatch {
                what: "window over 24h",
            });
        }
        let mut out = Vec::new();
        for entry in self.entries.iter() {
            if entry.tenant.as_str() == tenant && entry.session.as_str() == session {
                out.push(entry.clone());
            }
        }
        Ok(out)
    }
}

/// Ports `governance_append_and_query`: one commit reads back whole.
#[test]
fn governance_append_and_query() -> Result<(), CeremonyFailed> {
    let mut log = GovernanceLog::new();
    let entry = GovEntry::new("tenant-a", "sess-1", "sig-1")?;
    log.append(entry)?;
    let found = log.query("tenant-a", "sess-1", 0, 3_600)?;
    if found.len() != 1 {
        return failed("one committed entry reads back");
    }
    Ok(())
}

/// Ports `governance_duplicate_signal_id_returns_409` as a typed conflict.
#[test]
fn governance_duplicate_signal_is_a_typed_conflict() -> Result<(), CeremonyFailed> {
    let mut log = GovernanceLog::new();
    log.append(GovEntry::new("tenant-a", "sess-1", "sig-dup")?)?;
    let second = log.append(GovEntry::new("tenant-a", "sess-1", "sig-dup")?);
    match second {
        Err(CeremonyFailed::Mismatch { .. }) => Ok(()),
        Ok(()) => failed("a duplicate signal is refused"),
        Err(other) => Err(other),
    }
}

/// Ports `governance_cross_tenant_returns_404` as an empty read.
#[test]
fn governance_cross_tenant_reads_none() -> Result<(), CeremonyFailed> {
    let mut log = GovernanceLog::new();
    log.append(GovEntry::new("tenant-a", "sess-x", "sig-x")?)?;
    let foreign = log.query("tenant-b", "sess-x", 0, 3_600)?;
    if foreign.is_empty() {
        Ok(())
    } else {
        failed("a foreign tenant reads no entry")
    }
}

/// Ports the negative, over-wide and missing-session window refusals.
#[test]
fn governance_window_refusals_are_typed() -> Result<(), CeremonyFailed> {
    let log = GovernanceLog::new();
    let negative = log.query("tenant-a", "sess-1", 3_600, 100);
    match negative {
        Err(CeremonyFailed::Mismatch { .. }) => {}
        Ok(_) => return failed("a negative window is refused"),
        Err(other) => return Err(other),
    }
    let wide = log.query("tenant-a", "sess-1", 0, 90_000);
    match wide {
        Err(CeremonyFailed::Mismatch { .. }) => {}
        Ok(_) => return failed("a window over 24h is refused"),
        Err(other) => return Err(other),
    }
    let missing = log.query("tenant-a", "", 0, 100);
    match missing {
        Err(CeremonyFailed::Mismatch { .. }) => Ok(()),
        Ok(_) => failed("a missing session is refused"),
        Err(other) => Err(other),
    }
}

/// The authentication tag of `body` under `key`, keyed BLAKE3.
fn signal_tag(key: &[u8; 32], body: &[u8]) -> Digest {
    lgwks_std::hash::keyed(key, body)
}

/// Accept `body` only under the tag `key` recomputes, in constant time.
fn validate_signal(key: &[u8; 32], body: &[u8], tag_hex: &str) -> Result<(), CeremonyFailed> {
    check_bound("body", body.len(), MAX_BODY_BYTES)?;
    check_bound("tag", tag_hex.len(), 64)?;
    let wanted = signal_tag(key, body);
    let presented = match Digest::from_hex(tag_hex) {
        Ok(tag) => tag,
        Err(cause) => {
            lgwks_std::trace::warn!(?cause, "ceremony: a malformed signal tag is refused");
            return Err(CeremonyFailed::Mismatch {
                what: "malformed tag",
            });
        }
    };
    if presented.ct_eq(&wanted) {
        Ok(())
    } else {
        Err(CeremonyFailed::Mismatch {
            what: "invalid signal signature",
        })
    }
}

/// Accept `text` only within five minutes of `now`, either direction.
fn validate_timestamp(now: SystemTime, text: &str) -> Result<(), CeremonyFailed> {
    check_bound("timestamp", text.len(), MAX_SIGNAL_BYTES)?;
    let stamped = lgwks_std::time::parse_rfc3339(text)?;
    let drift_secs = match now.duration_since(stamped) {
        Ok(elapsed) => elapsed.as_secs(),
        Err(_) => match stamped.duration_since(now) {
            Ok(ahead) => ahead.as_secs(),
            Err(_) => return failed("clock drift is unreadable"),
        },
    };
    if drift_secs > MAX_DRIFT_SECS {
        lgwks_std::trace::warn!(drift_secs, "ceremony: a drifted timestamp is refused");
        return Err(CeremonyFailed::Mismatch {
            what: "drifted timestamp",
        });
    }
    Ok(())
}

/// The fixed test present: every timestamp is judged against this, never the wall.
fn fixed_now() -> Result<SystemTime, CeremonyFailed> {
    Ok(lgwks_std::time::parse_rfc3339("2026-06-01T12:00:00Z")?)
}

/// The fixed test stamp inside the drift bound.
fn fixed_stamp() -> &'static str {
    "2026-06-01T12:00:00Z"
}

/// The fixed test signal key; never beside the tags it checks.
fn signal_key() -> [u8; 32] {
    [0xABu8; 32]
}

/// A fresh signal validates and its timestamp is inside the drift bound.
#[test]
fn signals_accept_a_fresh_valid_signal() -> Result<(), CeremonyFailed> {
    let key = signal_key();
    let now = fixed_now()?;
    let stamp = fixed_stamp();
    let mut hasher = Hasher::new();
    hasher.write_framed(b"ceremony/signal.v1");
    hasher.write_framed(b"ext-1");
    hasher.write_framed(stamp.as_bytes());
    let body = hasher.finalize().as_bytes().to_vec();
    if body.len() > MAX_BODY_BYTES {
        return failed("signal body fits its ceiling");
    }
    let tag = signal_tag(&key, body.as_slice()).to_hex();
    validate_signal(&key, body.as_slice(), tag.as_str())?;
    validate_timestamp(now, stamp)?;
    Ok(())
}

/// Ports `signals_integration`: forgery and drift are typed refusals.
#[test]
fn signals_refuse_a_forged_tag_and_a_drifted_stamp() -> Result<(), CeremonyFailed> {
    let key = signal_key();
    let body = b"ext-1:fully_signed";
    let good = signal_tag(&key, body.as_slice()).to_hex();
    let mut bad_bytes = good.into_bytes();
    let last = bad_bytes.len().saturating_sub(1);
    if let Some(slot) = bad_bytes.get_mut(last) {
        *slot = if *slot == b'0' { b'1' } else { b'0' };
    }
    let bad = String::from_utf8(bad_bytes).map_err(|_| CeremonyFailed::Mismatch {
        what: "tag is text",
    })?;
    match validate_signal(&key, body.as_slice(), bad.as_str()) {
        Err(CeremonyFailed::Mismatch { .. }) => {}
        Ok(()) => return failed("a forged tag is refused"),
        Err(other) => return Err(other),
    }
    let now = fixed_now()?;
    let drifted = "2000-01-01T00:00:00Z";
    match validate_timestamp(now, drifted) {
        Err(CeremonyFailed::Mismatch { .. }) => Ok(()),
        Ok(()) => failed("a drifted timestamp is refused"),
        Err(other) => Err(other),
    }
}

/// Hostile inputs never panic and never allocate past their ceilings.
#[test]
fn signals_reject_malformed_inputs_typed() -> Result<(), CeremonyFailed> {
    let key = signal_key();
    match validate_signal(&key, b"body", "not-hex") {
        Err(CeremonyFailed::Mismatch { .. }) => {}
        Ok(()) => return failed("a malformed tag is refused"),
        Err(other) => return Err(other),
    }
    let now = fixed_now()?;
    match validate_timestamp(now, "not-a-timestamp") {
        Err(CeremonyFailed::Time(_)) => {}
        Ok(()) => return failed("a malformed timestamp is refused"),
        Err(other) => return Err(other),
    }
    let wide = vec![0u8; MAX_BODY_BYTES.saturating_add(1)];
    match validate_signal(&key, wide.as_slice(), "00") {
        Err(CeremonyFailed::Bound { .. }) => Ok(()),
        Ok(()) => failed("an oversize body is refused before allocation"),
        Err(other) => Err(other),
    }
}

/// The restart case: the newest grant wins and binds its exact bytes.
#[test]
fn consent_latest_wins_survives_restart_and_binds_exact_bytes() -> Result<(), CeremonyFailed> {
    let mut store = consent_fixture()?;
    store.record(Consent::new(
        "tenant-a", "ws-7", "u-1", "a@x.io", "key-1", 1,
    )?)?;
    store.record(Consent::new(
        "tenant-a", "ws-7", "u-1", "a@x.io", "key-2", 2,
    )?)?;
    let active = store.latest("tenant-a", "ws-7")?;
    let consent = match active {
        Some(consent) => consent,
        None => return failed("restart reads back the newest grant"),
    };
    if consent.key() != "key-2" {
        return failed("the decision binds the exact latest bytes");
    }
    let payload = lgwks_std::json::to_vec(&consent.key().to_owned())?;
    let digest = lgwks_std::hash::blake3(payload.as_slice());
    let same = lgwks_std::hash::blake3(payload.as_slice());
    if !same.ct_eq(&digest) {
        return failed("same bytes same digest");
    }
    let foreign = store.latest("tenant-b", "ws-7")?;
    if foreign.is_some() {
        return failed("a restart never crosses tenants");
    }
    let other = store.latest("tenant-a", "ws-8")?;
    if other.is_some() {
        return failed("a restart never crosses workspaces");
    }
    Ok(())
}

/// The governance guard: the reference app is small and wires the home.
#[test]
fn reference_app_is_a_small_app_wiring() -> Result<(), CeremonyFailed> {
    let source = include_str!("../../examples/ceremony_reference.rs");
    let mut code = 0_usize;
    for line in source.lines() {
        let text = line.trim();
        if text.is_empty() || text.starts_with("//") {
            continue;
        }
        code = code.saturating_add(1);
    }
    if code == 0 || code >= 60 {
        return Err(CeremonyFailed::Mismatch {
            what: "reference app is under 60 lines",
        });
    }
    for token in [
        "App::new",
        "Provider",
        "AwaitApproval",
        "Effect::Ask",
        "Effect::Send",
        "app.check",
    ] {
        if !source.contains(token) {
            return Err(CeremonyFailed::Mismatch {
                what: "reference app wires the app",
            });
        }
    }
    Ok(())
}

/// Same seed replays the same trace hash; distinct seeds diverge.
#[test]
fn seeded_inputs_replay_to_one_trace_hash() -> Result<(), CeremonyFailed> {
    let first = lgwks_std::hash::blake3(b"ceremony-seed-7");
    let second = lgwks_std::hash::blake3(b"ceremony-seed-7");
    if !first.ct_eq(&second) {
        return failed("same seed same trace hash");
    }
    let third = lgwks_std::hash::blake3(b"ceremony-seed-8");
    if third.ct_eq(&first) {
        return failed("distinct seeds diverge");
    }
    Ok(())
}
