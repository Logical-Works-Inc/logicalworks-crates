//! The dependency report: one command that prints every edge's mortgage, every
//! deferral's trigger, every pin's expiry and every blocked provider's cover.
//!
//! `lgwks-deps check` refuses; this report explains. It renders the same
//! registers the gate reads — the approvals, the deferral catalog and the
//! supply-chain ledger — beside the resolved lockfile, as human text and as
//! JSON under stable keys. A quarterly review reads this page and closes or
//! re-affirms each edge; the JSON is what a later lane would diff.

use std::fmt;

use crate::{
    contract::Contract,
    deferral::Catalog,
    lifecycle::Lifecycle,
    lock::Resolved,
    patches::{BLOCKED_PROVIDERS, Ledger, via_chain},
};

// ── Report ──────────────────────────────────────────────────────────────────

/// Why a report could not be rendered.
///
/// Rendering writes into a `String`, which cannot fail for lack of space; the
/// variant carries the formatter's own error so the refusal names what failed
/// instead of discarding it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LedgerError {
    /// String rendering failed.
    Render {
        /// What the formatter reported.
        cause: String,
    },
}

impl fmt::Display for LedgerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Render { ref cause } => write!(
                formatter,
                "the dependency report could not be rendered: {cause}"
            ),
        }
    }
}

impl std::error::Error for LedgerError {}

/// One rendered line of the report.
fn line(out: &mut String, arguments: fmt::Arguments<'_>) -> Result<(), LedgerError> {
    use std::fmt::Write as _;
    writeln!(out, "{arguments}").map_err(|cause| LedgerError::Render {
        cause: cause.to_string(),
    })
}

/// Appends rendered arguments to the report without a trailing newline.
fn push(out: &mut String, arguments: fmt::Arguments<'_>) -> Result<(), LedgerError> {
    use std::fmt::Write as _;
    out.write_fmt(arguments)
        .map_err(|cause| LedgerError::Render {
            cause: cause.to_string(),
        })
}

/// Quotes `value` as one JSON string, escaping through the estate facade.
///
/// The ledger never hand-rolls string escaping: `lgwks_std::json` owns JSON
/// encoding, so a reason carrying quotes, backslashes or control characters
/// cannot break the document.
fn quoted(value: &str) -> Result<String, LedgerError> {
    lgwks_std::json::to_string(&value).map_err(|cause| LedgerError::Render {
        cause: cause.to_string(),
    })
}

// ── Report ──────────────────────────────────────────────────────────────────

/// Everything one report renders, borrowed from the gate's own inputs.
#[derive(Debug, Clone, Copy)]
#[non_exhaustive]
pub struct Input<'a> {
    /// The parsed approval register.
    pub register: &'a Contract,
    /// The parsed deferral catalog, when the repository carries one.
    pub deferral: Option<&'a Catalog>,
    /// The parsed supply-chain ledger, when the repository carries one.
    pub supply: Option<&'a Ledger>,
    /// The parsed lifecycle ledger, when the repository carries one.
    pub lifecycle: Option<&'a Lifecycle>,
    /// The resolved lockfile packages.
    pub locked: &'a [Resolved],
    /// ISO date the report is read on: production passes today, tests pass a
    /// fixed date, so ages never depend on the machine's clock.
    pub today: &'a str,
}

impl<'a> Input<'a> {
    /// Borrows everything one report renders.
    #[must_use]
    pub fn new(
        register: &'a Contract,
        deferral: Option<&'a Catalog>,
        supply: Option<&'a Ledger>,
        lifecycle: Option<&'a Lifecycle>,
        locked: &'a [Resolved],
        today: &'a str,
    ) -> Self {
        Self {
            register,
            deferral,
            supply,
            lifecycle,
            locked,
            today,
        }
    }
}

/// Renders the report as human text: one section per register, one line per
/// row, with the mortgage beside every edge.
pub fn render_text(input: &Input<'_>) -> Result<String, LedgerError> {
    let mut out = String::new();
    line(&mut out, format_args!("ledger {}", input.today))?;
    line(
        &mut out,
        format_args!("edges {}", input.register.entry_count()),
    )?;
    for entry in input.register.approvals() {
        let mut replacement = "-";
        let mut removal = "-";
        let mut reviewed = "never";
        if let Some(row) = input
            .lifecycle
            .and_then(|ledger| ledger.row_for(entry.krate()))
        {
            replacement = row.replacement();
            removal = row.removal_trigger();
            reviewed = row.last_reviewed();
        }
        line(
            &mut out,
            format_args!(
                "edge {} owner {} consumers {} replacement {} removal {} reviewed {}",
                entry.krate(),
                entry.owner(),
                entry.allowed_consumers.join(","),
                replacement,
                removal,
                reviewed,
            ),
        )?;
    }
    if let Some(catalog) = input.deferral {
        line(&mut out, format_args!("deferred {}", catalog.entry_count()))?;
        for row in catalog.entries() {
            line(
                &mut out,
                format_args!(
                    "defer {} family {} owner {} trigger {} review {}",
                    row.krate(),
                    row.family,
                    row.owner(),
                    row.trigger(),
                    row.review(),
                ),
            )?;
        }
    }
    if let Some(ledger) = input.supply {
        line(&mut out, format_args!("pins {}", ledger.entries().len()))?;
        for pin in ledger.entries() {
            let state = if pin.is_live(input.today) {
                "live"
            } else {
                "expired"
            };
            line(
                &mut out,
                format_args!(
                    "pin {} {} owner {} expires {} {state} upstream {}",
                    pin.krate,
                    kind_label(pin.kind()),
                    pin.owner,
                    pin.expires_on(),
                    match pin.upstream_issue.as_deref().or(pin.upstream_pr.as_deref()) {
                        Some(upstream) => upstream,
                        None => pin.review.as_str(),
                    },
                ),
            )?;
        }
    }
    line(
        &mut out,
        format_args!("providers {}", BLOCKED_PROVIDERS.len()),
    )?;
    for blocked in BLOCKED_PROVIDERS {
        match input.locked.iter().find(|item| item.name() == blocked) {
            None => {
                line(&mut out, format_args!("provider {blocked} absent"))?;
            }
            Some(package) => {
                let covered = provider_covered(input, blocked);
                line(
                    &mut out,
                    format_args!(
                        "provider {blocked} {} via {} {}",
                        package.version(),
                        via_chain(input.locked, blocked),
                        if covered { "covered" } else { "UNCOVERED" },
                    ),
                )?;
            }
        }
    }
    Ok(out)
}

/// Renders an optional string field: the quoted value, or null.
fn field(value: Option<&str>) -> Result<String, LedgerError> {
    match value {
        Some(text) => quoted(text),
        None => Ok("null".to_owned()),
    }
}

/// Whether a live provider exception in the supply-chain ledger covers the
/// blocked provider `blocked`.
fn provider_covered(input: &Input<'_>, blocked: &str) -> bool {
    input.supply.is_some_and(|ledger| {
        ledger.entries().iter().any(|pin| {
            pin.kind() == crate::patches::PinKind::ProviderException
                && pin.krate() == blocked
                && pin.is_live(input.today)
        })
    })
}

/// The report's spelling of a pin kind, the same in text and JSON.
fn kind_label(kind: crate::patches::PinKind) -> &'static str {
    match kind {
        crate::patches::PinKind::UpstreamPin => "upstream-pin",
        crate::patches::PinKind::ProviderException => "provider-exception",
    }
}

/// The mortgage replacement for `krate`, when the lifecycle ledger names one.
fn mortgage_replacement<'a>(input: &'a Input<'_>, krate: &str) -> Option<&'a str> {
    input
        .lifecycle
        .and_then(|ledger| ledger.row_for(krate))
        .map(crate::lifecycle::Mortgage::replacement)
}

/// The mortgage removal trigger for `krate`, when the lifecycle ledger names one.
fn mortgage_trigger<'a>(input: &'a Input<'_>, krate: &str) -> Option<&'a str> {
    input
        .lifecycle
        .and_then(|ledger| ledger.row_for(krate))
        .map(crate::lifecycle::Mortgage::removal_trigger)
}

/// The mortgage review date for `krate`, when the lifecycle ledger names one.
fn mortgage_reviewed<'a>(input: &'a Input<'_>, krate: &str) -> Option<&'a str> {
    input
        .lifecycle
        .and_then(|ledger| ledger.row_for(krate))
        .map(crate::lifecycle::Mortgage::last_reviewed)
}

/// Renders the report as one JSON object under stable keys.
///
/// The shape is additive: new sections arrive as new keys, and no key is ever
/// renamed, so a consumer that diffs two reports diffs the registers, not the
/// renderer.
pub fn render_json(input: &Input<'_>) -> Result<String, LedgerError> {
    let mut out = String::from("{");
    push(&mut out, format_args!("\"today\":{}", quoted(input.today)?))?;
    out.push_str(",\"edges\":[");
    let mut first = true;
    for entry in input.register.approvals() {
        if !first {
            out.push(',');
        }
        first = false;
        let mut consumers = String::new();
        for (index, name) in entry.allowed_consumers.iter().enumerate() {
            if index > 0 {
                consumers.push(',');
            }
            consumers.push_str(&quoted(name)?);
        }
        push(
            &mut out,
            format_args!(
                "{{\"crate\":{},\"owner\":{},\"capability\":{},\"consumers\":[{consumers}],\"tier\":\"{}\",\"version\":{},\"license\":{},\"replacement\":{},\"removal_trigger\":{},\"last_reviewed\":{}}}",
                quoted(entry.krate())?,
                quoted(entry.owner())?,
                quoted(&entry.capability)?,
                entry.tier(),
                quoted(&entry.version)?,
                quoted(entry.license())?,
                field(mortgage_replacement(input, entry.krate()))?,
                field(mortgage_trigger(input, entry.krate()))?,
                field(mortgage_reviewed(input, entry.krate()))?,
            ),
        )?;
    }
    out.push_str("],\"deferred\":[");
    first = true;
    if let Some(catalog) = input.deferral {
        for row in catalog.entries() {
            if !first {
                out.push(',');
            }
            first = false;
            push(
                &mut out,
                format_args!(
                    "{{\"crate\":{},\"family\":{},\"owner\":{},\"trigger\":{},\"replacement\":{},\"review\":{},\"deferred_on\":{}}}",
                    quoted(row.krate())?,
                    quoted(&row.family)?,
                    quoted(row.owner())?,
                    quoted(row.trigger())?,
                    quoted(row.replacement())?,
                    quoted(row.review())?,
                    quoted(row.deferred_on())?,
                ),
            )?;
        }
    }
    out.push_str("],\"pins\":[");
    first = true;
    if let Some(ledger) = input.supply {
        for pin in ledger.entries() {
            if !first {
                out.push(',');
            }
            first = false;
            push(
                &mut out,
                format_args!(
                    "{{\"crate\":{},\"kind\":\"{}\",\"owner\":{},\"expires_on\":{},\"live\":{},\"upstream\":{},\"recorded_on\":{},\"version\":{},\"patch_digest\":{},\"blast_radius\":{},\"removal_trigger\":{}}}",
                    quoted(pin.krate())?,
                    kind_label(pin.kind()),
                    quoted(&pin.owner)?,
                    quoted(pin.expires_on())?,
                    pin.is_live(input.today),
                    field(pin.upstream_pr.as_deref().or(pin.upstream_issue.as_deref()))?,
                    quoted(&pin.recorded_on)?,
                    field(pin.version.as_deref())?,
                    field(pin.patch_digest.as_deref())?,
                    field(pin.blast_radius.as_deref())?,
                    field(pin.removal_trigger.as_deref())?,
                ),
            )?;
        }
    }
    out.push_str("],\"providers\":[");
    first = true;
    for blocked in BLOCKED_PROVIDERS {
        if !first {
            out.push(',');
        }
        first = false;
        match input.locked.iter().find(|item| item.name() == blocked) {
            None => {
                push(
                    &mut out,
                    format_args!("{{\"crate\":\"{blocked}\",\"present\":false}}"),
                )?;
            }
            Some(package) => {
                let covered = provider_covered(input, blocked);
                push(
                    &mut out,
                    format_args!(
                        "{{\"crate\":\"{blocked}\",\"present\":true,\"version\":{},\"via\":{},\"covered\":{covered}}}",
                        quoted(package.version())?,
                        quoted(&via_chain(input.locked, blocked))?,
                    ),
                )?;
            }
        }
    }
    out.push_str("]}");
    Ok(out)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// What every test here returns.
    ///
    /// The workspace forbids `unwrap`/`expect` outright, with no test exemption,
    /// so a test propagates its failure with `?` rather than aborting the run.
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn register() -> Result<Contract, Box<dyn std::error::Error>> {
        register_at("^0.8")
    }

    /// The one-edge register with `requirement` as its approved version.
    fn register_at(requirement: &str) -> Result<Contract, Box<dyn std::error::Error>> {
        let head = format!(
            "[[approved]]\ncrate = \"axum\"\ntier = \"boundary\"\nversion = \"{requirement}\"\n"
        );
        let text = concat!(
            "owner = \"lgwks_deps\"\n",
            "capability = \"http.server\"\n",
            "license = \"MIT\"\n",
            "source = \"registry\"\n",
            "allowed_consumers = \"lgwks_deps\"\n",
            "allowed_kinds = \"normal\"\n",
            "reason = \"Revenue slice HTTP routing needs Tower-compatible routing the standard library does not provide.\"\n",
            "approved_by = \"maintainer\"\n",
            "approved_on = \"2026-10-26\"\n",
            "review = \"https://github.com/Logical-Works-Inc/logicalworks-crates/issues/721\"\n",
        );
        Ok(Contract::parse(&format!("{head}{text}"))?)
    }

    /// One supply-chain ledger row of `kind` for axum, with the fields that
    /// kind requires.
    fn ledger_row(kind: &str) -> Result<Ledger, Box<dyn std::error::Error>> {
        let fields = if kind == "upstream-pin" {
            concat!(
                "version = \"0.8.9\"\n",
                "upstream_issue = \"https://example.invalid/upstream/issues/1\"\n",
                "upstream_pr = \"https://example.invalid/upstream/pull/2\"\n",
                "patch_digest = \"sha256:0000000000000000000000000000000000000000000000000000000000000000\"\n",
            )
        } else {
            concat!(
                "blast_radius = \"The storefront's serve feature only, and no other edge.\"\n",
                "removal_trigger = \"The next release resolves without the excused crate.\"\n",
            )
        };
        let text = format!(
            concat!(
                "[[patch]]\n",
                "crate = \"axum\"\n",
                "kind = \"{}\"\n",
                "owner = \"lgwks_deps\"\n",
                "reason = \"The newest release fails to build, so the edge holds one release.\"\n",
                "review = \"https://example.invalid/upstream/issues/1\"\n",
                "recorded_on = \"2026-11-12\"\n",
                "expires_on = \"2027-02-12\"\n",
                "{}",
            ),
            kind, fields
        );
        Ok(Ledger::parse(&text)?)
    }

    /// The registry approvals that hold one exact release (`=x.y.z`) with no
    /// upstream pin in the supply-chain ledger to excuse them, by crate name.
    ///
    /// The gate holds each manifest requirement equal to its approval
    /// (`RequirementDrift`), so reading the register reads every manifest.
    fn unexcused_exact_pins<'a>(register: &'a Contract, pins: &Ledger) -> Vec<&'a str> {
        let mut offenders: Vec<&str> = register
            .approvals()
            .filter(|entry| entry.source == "registry" && entry.version.trim().starts_with('='))
            .map(crate::contract::Entry::krate)
            .filter(|krate| {
                !pins.entries().iter().any(|pin| {
                    pin.kind() == crate::patches::PinKind::UpstreamPin && pin.krate() == *krate
                })
            })
            .collect();
        offenders.sort_unstable();
        offenders.dedup();
        offenders
    }

    /// Reads one committed register file from the repository's `contract/`.
    fn committed(name: &str) -> Result<String, Box<dyn std::error::Error>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contract")
            .join(name);
        Ok(std::fs::read_to_string(&path)
            .map_err(|cause| format!("cannot read {}: {cause}", path.display()))?)
    }

    fn lifecycle() -> Result<crate::lifecycle::Lifecycle, Box<dyn std::error::Error>> {
        let text = concat!(
            "[[lifecycle]]\n",
            "crate = \"axum\"\n",
            "replacement = \"An estate HTTP facade in lgwks_std covering routing with equal p99.\"\n",
            "removal_trigger = \"The estate router covers the same routes with equal p99.\"\n",
            "last_reviewed = \"2026-10-26\"\n",
            "review = \"https://github.com/Logical-Works-Inc/logicalworks-crates/issues/721\"\n",
        );
        Ok(crate::lifecycle::Lifecycle::parse(text)?)
    }

    fn input<'a>(
        register: &'a Contract,
        locked: &'a [Resolved],
        deferral: Option<&'a Catalog>,
        supply: Option<&'a Ledger>,
        lifecycle: Option<&'a crate::lifecycle::Lifecycle>,
    ) -> Input<'a> {
        Input::new(register, deferral, supply, lifecycle, locked, "2026-10-26")
    }

    #[test]
    fn the_text_report_names_every_mortgage() -> TestResult {
        let register = register()?;
        let lifecycle = lifecycle()?;
        let text = render_text(&input(&register, &[], None, None, Some(&lifecycle)))?;
        assert!(
            text.contains("edge axum owner lgwks_deps"),
            "the report must name the edge and its owner:\n{text}"
        );
        assert!(
            text.contains("An estate HTTP facade in lgwks_std covering routing with equal p99."),
            "the report must print the replacement beside the edge:\n{text}"
        );
        assert!(
            text.contains("provider ring absent"),
            "an absent provider must read as absent:\n{text}"
        );
        Ok(())
    }

    #[test]
    fn the_json_report_holds_stable_keys() -> TestResult {
        let register = register()?;
        let lifecycle = lifecycle()?;
        let document = render_json(&input(&register, &[], None, None, Some(&lifecycle)))?;
        for key in [
            "\"today\"",
            "\"edges\"",
            "\"deferred\"",
            "\"pins\"",
            "\"providers\"",
            "\"crate\":\"axum\"",
            "\"replacement\":\"An estate HTTP facade",
            "\"last_reviewed\":\"2026-10-26\"",
        ] {
            assert!(
                document.contains(key),
                "the JSON report must carry {key}:\n{document}"
            );
        }
        Ok(())
    }

    #[test]
    fn an_exact_pin_is_named_unless_an_upstream_pin_excuses_it() -> TestResult {
        let none = Ledger::parse("")?;
        let exact = register_at("=0.8.9")?;
        assert_eq!(unexcused_exact_pins(&exact, &none), ["axum"]);
        assert_eq!(
            unexcused_exact_pins(&exact, &ledger_row("provider-exception")?),
            ["axum"],
            "a provider exception excuses a provider, never a pin"
        );
        assert!(unexcused_exact_pins(&exact, &ledger_row("upstream-pin")?).is_empty());
        for open in ["^0.8.9", "0.8", "~0.8.9", ">=0.8, <0.9"] {
            assert!(
                unexcused_exact_pins(&register_at(open)?, &none).is_empty(),
                "{open} admits more than one release, so it is no pin"
            );
        }
        Ok(())
    }

    #[test]
    fn the_committed_register_pins_no_registry_edge_exactly() -> TestResult {
        let register = Contract::parse(&committed("APPROVED.toml")?)?;
        let pins = Ledger::parse(&committed("PATCHES.toml")?)?;
        let offenders = unexcused_exact_pins(&register, &pins);
        assert!(
            offenders.is_empty(),
            "an exact requirement turns every release into a manifest and register edit \
             (ADR 0031): write a caret requirement, or record an expiring upstream-pin row in \
             contract/PATCHES.toml, for {offenders:?}"
        );
        Ok(())
    }

    #[test]
    fn the_committed_register_names_no_product_owner() -> TestResult {
        let register = Contract::parse(&committed("APPROVED.toml")?)?;
        let mut offenders: Vec<&str> = register
            .approvals()
            .map(|entry| entry.owner())
            .filter(|owner| !["lgwks_deps", "lgwks_std", "lgwks_ast"].contains(owner))
            .collect();
        offenders.sort_unstable();
        offenders.dedup();
        assert!(
            offenders.is_empty(),
            "zero direct external edges: these owners are product crates, not the storefront: {offenders:?}"
        );
        Ok(())
    }
}
