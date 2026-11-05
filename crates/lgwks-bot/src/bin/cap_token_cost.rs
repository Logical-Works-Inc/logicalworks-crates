//! Matched-workload measurement: what bearer-token verify and policy decide cost.
//!
//! Two measurements over the same token and request, because a comparison
//! across different payloads measures the payloads:
//!
//! 1. `verify` — offline chain recomputation plus expiry and revocation on the
//!    injected clock.
//! 2. `decide` — the same verify folded into a deterministic decision with a
//!    trace over the one resource hierarchy.
//!
//! This is a measurement harness, not a pass/fail gate: wall time varies with
//! the machine, the shape of the distribution does not.
//!
//! ```text
//! cargo run -p lgwks_bot --bin cap_token_cost
//! ```
//!
//! The output is written through `std::io::Write` rather than `println!`
//! because `clippy::print_stdout` is forbidden workspace-wide with no
//! example-only carve-out.

use std::fmt;
use std::io::Write;
use std::time::{Duration, Instant};

use lgwks_bot::cap_token::{
    Authority, BearerToken, Caveat, Decision, MAX_CAVEATS, MAX_REVOKED, MAX_TEXT_LEN,
    MAX_TOKEN_BYTES, MAX_TRACE, POLICY_VERSION, Policy, Request, RevocationList, TokenError,
    TraceStep, WORKLOAD_ACTION,
};
use lgwks_bot::clock::Clock;

#[path = "../../examples/support/measure.rs"]
mod measure;

/// How many samples each measurement takes.
const SAMPLES: usize = 2_000;

/// Why a benchmark run failed: every arm names the typed cause, so a caller
/// matches the failure instead of parsing a message.
#[derive(Debug)]
enum BenchError {
    /// The token operation behind the measurement refused.
    Token(lgwks_bot::cap_token::TokenError),
    /// The `ps` output was not UTF-8.
    Utf8(std::string::FromUtf8Error),
    /// A count in the `ps` output did not parse.
    Parse(std::num::ParseIntError),
    /// A `ps` run failed.
    Io(std::io::Error),
    /// A benchmark invariant broke, in fixed words a caller can match.
    Check(&'static str),
}

impl fmt::Display for BenchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Token(ref cause) => write!(formatter, "the token operation refused: {cause}"),
            Self::Utf8(ref cause) => write!(formatter, "ps answered non-UTF-8 output: {cause}"),
            Self::Parse(ref cause) => write!(formatter, "ps answered an unparsable count: {cause}"),
            Self::Io(ref cause) => write!(formatter, "a ps run failed: {cause}"),
            Self::Check(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for BenchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match *self {
            Self::Token(ref cause) => Some(cause),
            Self::Utf8(ref cause) => Some(cause),
            Self::Parse(ref cause) => Some(cause),
            Self::Io(ref cause) => Some(cause),
            Self::Check(_) => None,
        }
    }
}

impl From<lgwks_bot::cap_token::TokenError> for BenchError {
    fn from(cause: lgwks_bot::cap_token::TokenError) -> Self {
        Self::Token(cause)
    }
}

impl From<std::string::FromUtf8Error> for BenchError {
    fn from(cause: std::string::FromUtf8Error) -> Self {
        Self::Utf8(cause)
    }
}

impl From<std::num::ParseIntError> for BenchError {
    fn from(cause: std::num::ParseIntError) -> Self {
        Self::Parse(cause)
    }
}

impl From<std::io::Error> for BenchError {
    fn from(cause: std::io::Error) -> Self {
        Self::Io(cause)
    }
}

/// Log `error` and hand it back, so every refusal leaves a record.
fn refused<T>(error: BenchError) -> Result<T, BenchError> {
    lgwks_std::trace::warn!(%error, "cap-token-cost: refused");
    Err(error)
}

/// The process peak resident set, in bytes.
///
/// Reads every process's resident set from `ps` and keeps the largest one
/// sharing this binary's name: with one load running at a time, that row is
/// this process.
fn peak_rss_bytes() -> Result<u64, BenchError> {
    let me = std::env::args()
        .next()
        .ok_or(BenchError::Check("argv has no entry"))?;
    let name = std::path::Path::new(me.as_str())
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(BenchError::Check("argv has no file name"))?;
    let output = std::process::Command::new("ps")
        .arg("-Ao")
        .arg("rss=,comm=")
        .output()?;
    let text = String::from_utf8(output.stdout)?;
    let mut peak = 0_u64;
    let mut seen = false;
    let mut rows = 0_usize;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        let (Some(rss_text), Some(comm)) = (words.next(), words.next()) else {
            continue;
        };
        rows = rows.saturating_add(1);
        if comm != name && !comm.ends_with(name) {
            continue;
        }
        seen = true;
        let kilobytes: u64 = rss_text.parse()?;
        let bytes = kilobytes
            .checked_mul(1_024)
            .ok_or(BenchError::Check("the resident set does not fit u64"))?;
        if bytes > peak {
            peak = bytes;
        }
    }
    if seen {
        Ok(peak)
    } else {
        refused(BenchError::Check("ps answered no row for this binary"))
    }
}

/// Report one measurement.
fn report(label: &str, summary: &measure::Summary) {
    let mut out = std::io::stdout().lock();
    let _written = writeln!(out, "{}", summary.line(label));
}

/// Walk every public operation once, untimed, so this binary is a real caller
/// of the surface it measures: workload issuance, revocation listing, expiry
/// readings, the trace walk and its hashes, each checked against its bound.
fn self_check(authority: &Authority, policy: &Policy, clock: &Clock) -> Result<(), BenchError> {
    let workload =
        authority.issue_workload("edge", "/edge/node-7", Duration::from_secs(20), clock)?;
    let caveat = workload
        .caveats()
        .first()
        .ok_or(BenchError::Check("a workload token carries its caveat"))?;
    if caveat.action() != WORKLOAD_ACTION {
        return refused(BenchError::Check(
            "a workload identity names the workload action",
        ));
    }
    if caveat.expires_at() <= clock.now() {
        return refused(BenchError::Check(
            "a fresh workload identity is already live",
        ));
    }
    if workload.caveats().len() != 1 {
        return refused(BenchError::Check("an issuance mints its one caveat"));
    }
    let mut listed = RevocationList::new();
    if !listed.is_empty() {
        return refused(BenchError::Check("a new revocation list is empty"));
    }
    listed.revoke(workload.id())?;
    if !listed.contains(workload.id()) {
        return refused(BenchError::Check("a revoked id reads back revoked"));
    }
    if listed.len() != 1 {
        return refused(BenchError::Check("the list holds its one revocation"));
    }
    if listed.len() > MAX_REVOKED {
        return refused(BenchError::Check("the list fits its bound"));
    }
    match authority.verify(&workload, &listed, clock) {
        Err(TokenError::Revoked) => (),
        _other => {
            return refused(BenchError::Check("a revoked workload fails verify typed"));
        }
    }
    let probe = Request::new("notes", "read", "/notes/alice/inbox")?;
    if probe.audience().len() > MAX_TEXT_LEN
        || probe.action().len() > MAX_TEXT_LEN
        || probe.resource().len() > MAX_TEXT_LEN
    {
        return refused(BenchError::Check("a request fits its text bound"));
    }
    let decision: Decision = policy.decide(authority, &workload, &probe, &listed, clock)?;
    if decision.allowed() {
        return refused(BenchError::Check("a revoked workload decides deny"));
    }
    let trace: &[TraceStep] = decision.trace();
    for step in trace {
        let _rule: &str = step.rule();
        let _version: u32 = step.version();
        let _inputs: &str = step.inputs();
        if step.version() != POLICY_VERSION {
            return refused(BenchError::Check("the trace carries the policy version"));
        }
    }
    if decision.trace().len() > MAX_TRACE {
        return refused(BenchError::Check("the trace fits its bound"));
    }
    if decision.inputs_hash().len() != 32 {
        return refused(BenchError::Check("the inputs hash is thirty-two bytes"));
    }
    let _trace_hash = decision.trace_hash();
    Ok(())
}

fn main() -> Result<(), BenchError> {
    let clock = Clock::virtual_at(Duration::from_secs(1_000));
    let authority = Authority::generate()?;
    let token = authority.mint(
        "notes",
        "read",
        "/notes",
        Duration::from_secs(3_600),
        &clock,
    )?;
    let narrowed = token.attenuate(&[Caveat::new(
        "notes",
        "read",
        "/notes/alice",
        Duration::from_secs(1_800),
    )?])?;
    let bytes = narrowed.to_bytes()?;
    if bytes.len() > MAX_TOKEN_BYTES {
        return refused(BenchError::Check("a token fits its encoding bound"));
    }
    if narrowed.caveats().len() > MAX_CAVEATS {
        return refused(BenchError::Check("a token fits its caveat bound"));
    }
    let token = BearerToken::from_bytes(bytes.as_slice())?;
    let request = Request::new("notes", "read", "/notes/alice/inbox")?;
    let policy = Policy::new();
    let revocations = RevocationList::new();
    self_check(&authority, &policy, &clock)?;

    let mut out = std::io::stdout().lock();
    let _written = writeln!(
        out,
        "capability tokens: policy version {POLICY_VERSION}, bounds caveats={MAX_CAVEATS} text={MAX_TEXT_LEN} bytes={MAX_TOKEN_BYTES}"
    );
    let (audience, action, resource) = match token.caveats().first() {
        Some(caveat) => (caveat.audience(), caveat.action(), caveat.resource()),
        None => ("none", "none", "none"),
    };
    let _written = writeln!(
        out,
        "token id={} audience={} action={} resource={} tag={} request aud={} act={} res={}",
        lgwks_std::hex::encode(token.id()),
        audience,
        action,
        resource,
        lgwks_std::hex::encode(token.tag()),
        request.audience(),
        request.action(),
        request.resource(),
    );
    drop(out);

    let slots = SAMPLES;
    let mut verifying = Vec::with_capacity(slots);
    for _ in 0..slots {
        let started = Instant::now();
        authority.verify(&token, &revocations, &clock)?;
        verifying.push(started.elapsed().as_micros());
    }
    report(
        "verify (offline chain)",
        &measure::Summary::of(&mut verifying),
    );

    let mut deciding = Vec::with_capacity(slots);
    for _ in 0..slots {
        let started = Instant::now();
        let decision: Decision =
            policy.decide(&authority, &token, &request, &revocations, &clock)?;
        if !decision.allowed() {
            return refused(BenchError::Check("the benchmark token must allow"));
        }
        deciding.push(started.elapsed().as_micros());
    }
    let summary = measure::Summary::of(&mut deciding);
    report("decide (with trace)", &summary);

    let peak = peak_rss_bytes()?;
    let mut out = std::io::stdout().lock();
    let _written = writeln!(out, "peak_rss_bytes={peak}");
    Ok(())
}
