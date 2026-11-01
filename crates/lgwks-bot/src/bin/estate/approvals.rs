//! Human approvals on the shipped artifact: list, approve, deny.
//!
//! `estate approvals <list|approve|deny> --store DIR --tenant T ...` drives
//! [`Host::decide`](lgwks_bot::task::Host::decide) and
//! [`Host::pending`](lgwks_bot::task::Host::pending) over the real file store,
//! so an operator decides exactly what a run parked. Every refusal is the
//! host's typed answer, printed rather than parsed.
//!
//! ```text
//! estate approvals list --store DIR --tenant acme [--limit 100] [--max-pending N] [--json]
//! estate approvals approve --store DIR --tenant acme --id <id> --digest <hex> --by <who> [--json]
//! estate approvals deny --store DIR --tenant acme --id <id> --digest <hex> --by <who> [--json]
//! ```
//!
//! A decision for another tenant's approval is refused and the refusal prints
//! the host's audit of the attempt.

use std::io::Write;

use lgwks_bot::script::{
    ApprovalDecision, ApprovalId, ApprovalVerdict, DecisionError, MAX_PENDING_APPROVALS,
    MAX_PENDING_PER_TENANT, PendingApproval,
};
use lgwks_bot::task::{Host, HostBuilder};

/// Why the approvals command could not run.
#[derive(Debug)]
pub(crate) enum ApprovalsError {
    /// The command line is wrong.
    Usage(String),
    /// The host could not be built or the store opened.
    Host(String),
    /// The decision or listing was refused.
    Refused(String),
}

impl std::fmt::Display for ApprovalsError {
    /// What was asked, and why it was refused.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::Usage(ref why) => write!(formatter, "estate approvals: {why}"),
            Self::Host(ref why) => write!(formatter, "estate approvals: the host failed: {why}"),
            Self::Refused(ref why) => write!(formatter, "estate approvals: refused: {why}"),
        }
    }
}

impl std::error::Error for ApprovalsError {}

/// The parsed command line.
struct Options {
    /// What to do: `list`, `approve` or `deny`.
    command: Command,
    /// The store directory.
    store: String,
    /// The tenant that parked.
    tenant: String,
    /// The approval id, for `approve` and `deny`.
    id: Option<String>,
    /// The digest the decision is made against, for `approve` and `deny`.
    digest: Option<String>,
    /// Who decides, for `approve` and `deny`.
    by: Option<String>,
    /// How many pending approvals `list` prints.
    limit: usize,
    /// The tenant's undecided-approval ceiling the host is built with.
    max_pending: usize,
    /// Machine output.
    json: bool,
}

/// What the command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    /// List undecided approvals.
    List,
    /// Approve one.
    Approve,
    /// Deny one.
    Deny,
}

/// Read `--flag value` or `--flag=value` after `args`, or refuse.
fn flag_value(args: &[String], flag: &str) -> Result<Option<String>, ApprovalsError> {
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        if let Some(inline) = arg.strip_prefix(&format!("--{flag}=")) {
            return Ok(Some(inline.to_owned()));
        }
        if arg == &format!("--{flag}") {
            match rest.next() {
                Some(value) => return Ok(Some(value.clone())),
                None => {
                    lgwks_std::trace::debug!(flag = %flag, "flag_value: the flag needs a value");
                    return Err(ApprovalsError::Usage(format!("`--{flag}` needs a value")));
                }
            }
        }
    }
    Ok(None)
}

/// Whether `args` names `flag` in either spelling.
fn has_flag(args: &[String], flag: &str) -> bool {
    args.iter().any(|arg| arg == &format!("--{flag}"))
}

/// Parse the approvals command line.
fn parse_options(args: &[String]) -> Result<Options, ApprovalsError> {
    let (command, rest) = match args.split_first() {
        Some((first, rest)) if first == "list" => (Command::List, rest),
        Some((first, rest)) if first == "approve" => (Command::Approve, rest),
        Some((first, rest)) if first == "deny" => (Command::Deny, rest),
        _ => {
            lgwks_std::trace::debug!(args = ?args, "parse_options: the command is not list, approve or deny");
            return Err(ApprovalsError::Usage(
                "expected one of `list`, `approve`, `deny`".to_owned(),
            ));
        }
    };
    let store = flag_value(rest, "store")?.ok_or_else(|| {
        ApprovalsError::Usage("`--store DIR` names the run-store directory".to_owned())
    })?;
    let tenant = flag_value(rest, "tenant")?.ok_or_else(|| {
        ApprovalsError::Usage("`--tenant T` names the tenant that parked".to_owned())
    })?;
    let id = flag_value(rest, "id")?;
    let digest = flag_value(rest, "digest")?;
    let by = flag_value(rest, "by")?;
    let limit = match flag_value(rest, "limit")? {
        Some(text) => text
            .parse::<usize>()
            .map_err(|error| {
                lgwks_std::trace::debug!(error = %error, "parse_options: `--limit` is not a whole number");
                ApprovalsError::Usage("`--limit N` takes a whole number".to_owned())
            })?,
        None => MAX_PENDING_PER_TENANT,
    };
    let max_pending = match flag_value(rest, "max-pending")? {
        Some(text) => match text.parse::<usize>() {
            Ok(ceiling) if (1..=MAX_PENDING_APPROVALS).contains(&ceiling) => ceiling,
            parsed => {
                lgwks_std::trace::debug!(
                    ?parsed,
                    "parse_options: `--max-pending` is outside 1..=MAX_PENDING_APPROVALS"
                );
                return Err(ApprovalsError::Usage(format!(
                    "`--max-pending N` takes a whole number from 1 to {MAX_PENDING_APPROVALS}"
                )));
            }
        },
        None => MAX_PENDING_PER_TENANT,
    };
    if matches!(command, Command::Approve | Command::Deny)
        && (id.is_none() || digest.is_none() || by.is_none())
    {
        lgwks_std::trace::debug!(
            ?command,
            ?id,
            ?digest,
            ?by,
            "parse_options: approve and deny need id, digest and by"
        );
        return Err(ApprovalsError::Usage(
            "`approve` and `deny` need `--id`, `--digest` and `--by`".to_owned(),
        ));
    }
    Ok(Options {
        command,
        store,
        tenant,
        id,
        digest,
        by,
        limit,
        max_pending,
        json: has_flag(rest, "json"),
    })
}

/// Escape `text` for one JSON string value.
fn escape(text: &str) -> Result<String, ApprovalsError> {
    let mut out = String::with_capacity(text.len().saturating_add(2));
    for point in text.chars() {
        match point {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            point if u32::from(point) < 0x20 => {
                std::fmt::Write::write_fmt(&mut out, format_args!("\\u{:04x}", u32::from(point)))
                    .map_err(|error| ApprovalsError::Host(error.to_string()))?;
            }
            _ => out.push(point),
        }
    }
    Ok(out)
}

/// Up to `limit` undecided approvals, or the host's refusal to list them.
fn listed(host: &Host, limit: usize) -> Result<Vec<PendingApproval>, ApprovalsError> {
    host.pending(limit).map_err(|error| {
        lgwks_std::trace::debug!(error = %error, "listed: the host refused the listing");
        ApprovalsError::Refused(error.to_string())
    })
}

/// The pending approvals as JSON.
fn list_json(host: &Host, limit: usize) -> Result<String, ApprovalsError> {
    let pending = listed(host, limit)?;
    let mut rows = String::from("[");
    for (index, entry) in pending.iter().enumerate() {
        if index > 0 {
            rows.push(',');
        }
        std::fmt::Write::write_fmt(
            &mut rows,
            format_args!(
                "{{\"id\":\"{}\",\"tenant\":\"{}\",\"path\":\"{}\",\"name\":\"{}\",\"digest\":\"{}\",\"payload_bytes\":{},\"expires_at_secs\":{}}}",
                escape(entry.id())?,
                escape(entry.tenant())?,
                escape(entry.path())?,
                escape(entry.name())?,
                entry.digest().to_hex(),
                entry.payload_len(),
                entry.expires_at().as_secs(),
            ),
        )
        .map_err(|error| ApprovalsError::Host(error.to_string()))?;
    }
    rows.push(']');
    Ok(rows)
}

/// List undecided approvals, human or machine readable.
fn list(host: &Host, options: &Options, out: &mut impl Write) -> Result<(), ApprovalsError> {
    if options.json {
        let json = list_json(host, options.limit)?;
        writeln!(out, "{json}").map_err(|error| ApprovalsError::Host(error.to_string()))?;
        return Ok(());
    }
    let pending = listed(host, options.limit)?;
    if pending.is_empty() {
        writeln!(out, "no pending approvals")
            .map_err(|error| ApprovalsError::Host(error.to_string()))?;
        return Ok(());
    }
    for entry in &pending {
        writeln!(
            out,
            "{} {} {} digest={} payload_bytes={} expires_in={:?}",
            entry.id(),
            entry.tenant(),
            entry.path(),
            entry.digest().to_hex(),
            entry.payload_len(),
            entry.expires_at(),
        )
        .map_err(|error| ApprovalsError::Host(error.to_string()))?;
    }
    Ok(())
}

/// The value of a flag a decision cannot be made without, or a usage refusal
/// naming it; an absent `--id`, `--by` or `--digest` is never an empty one.
fn required<'a>(value: Option<&'a str>, flag: &str) -> Result<&'a str, ApprovalsError> {
    match value {
        Some(value) => Ok(value),
        None => {
            lgwks_std::trace::debug!(flag = %flag, "required: a decision needs this flag");
            Err(ApprovalsError::Usage(format!(
                "a decision needs `--{flag}`"
            )))
        }
    }
}

/// Record an approval or a denial.
fn decide(host: &Host, options: &Options, out: &mut impl Write) -> Result<(), ApprovalsError> {
    // Read the id before the store is touched: a malformed id is the host's
    // own typed refusal, and the canonical spelling is what gets printed.
    let id_text = required(options.id.as_deref(), "id")?;
    let canonical = ApprovalId::parse(id_text)
        .map_err(|error| ApprovalsError::Refused(error.to_string()))?
        .to_string();
    let id = canonical.as_str();
    let by = required(options.by.as_deref(), "by")?;
    let digest_text = required(options.digest.as_deref(), "digest")?;
    let digest = lgwks_std::hash::Digest::from_hex(digest_text).map_err(|error| {
        ApprovalsError::Usage(format!("`--digest` is 64 hex characters, got: {error}"))
    })?;
    let verdict = match options.command {
        Command::Approve => ApprovalVerdict::Approve,
        Command::Deny => ApprovalVerdict::Deny,
        Command::List => {
            lgwks_std::trace::debug!("decide: a decision needs approve or deny, not list");
            return Err(ApprovalsError::Usage(
                "a decision needs `approve` or `deny`".to_owned(),
            ));
        }
    };
    let decision = match verdict {
        ApprovalVerdict::Approve => ApprovalDecision::approve(by, digest),
        _ => ApprovalDecision::deny(by, digest),
    };
    match host.decide(id, decision) {
        Ok(()) => {
            if options.json {
                writeln!(
                    out,
                    "{{\"id\":\"{}\",\"verdict\":\"{}\",\"by\":\"{}\"}}",
                    escape(id)?,
                    match verdict {
                        ApprovalVerdict::Approve => "approved",
                        _ => "denied",
                    },
                    escape(by)?,
                )
                .map_err(|error| ApprovalsError::Host(error.to_string()))?;
            } else {
                writeln!(
                    out,
                    "{} {} by {by}",
                    match verdict {
                        ApprovalVerdict::Approve => "approved",
                        _ => "denied",
                    },
                    id,
                )
                .map_err(|error| ApprovalsError::Host(error.to_string()))?;
            }
            Ok(())
        }
        Err(error) if matches!(error, DecisionError::ForeignTenant { .. }) => {
            let mut why = error.to_string();
            for entry in host.audit() {
                std::fmt::Write::write_fmt(
                    &mut why,
                    format_args!(
                        "; audited at {:?}: tenant {} asked for {} owned by {}",
                        entry.at(),
                        entry.tenant(),
                        entry.id(),
                        entry.owner()
                    ),
                )
                .map_err(|cause| ApprovalsError::Host(cause.to_string()))?;
            }
            lgwks_std::trace::debug!(why = %why, "decide: another tenant's approval, refused and audited");
            Err(ApprovalsError::Refused(why))
        }
        Err(error) => {
            lgwks_std::trace::debug!(error = %error, "decide: the host refused the decision");
            Err(ApprovalsError::Refused(error.to_string()))
        }
    }
}

/// Run `estate approvals ...`: list, approve or deny over the file store.
pub(crate) fn run(args: &[String], out: &mut impl Write) -> Result<(), ApprovalsError> {
    let options = parse_options(args)?;
    let builder: HostBuilder = Host::builder(&options.tenant)
        .map_err(|error| ApprovalsError::Host(error.to_string()))?
        .run_store(&options.store)
        .map_err(|error| ApprovalsError::Host(error.to_string()))?;
    let host = builder
        .max_pending_approvals(options.max_pending)
        .build()
        .map_err(|error| ApprovalsError::Host(error.to_string()))?;
    match options.command {
        Command::List => list(&host, &options, out),
        Command::Approve | Command::Deny => decide(&host, &options, out),
    }
}
